<script setup lang="ts">
import type { UnlistenFn } from '@tauri-apps/api/event'
import type { PlanSnapshot, PlanStatus } from '../../types/plan'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { useStorage } from '@vueuse/core'
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'

const props = defineProps<{
  workspace: string
}>()

const isCollapsed = useStorage('popup-plan-panel-collapsed', true)
const snapshot = ref<PlanSnapshot | null>(null)
const loading = ref(true)
const readError = ref('')
const watchError = ref('')
const eventError = ref('')
const completedAnimationId = ref('')
const localNote = ref('')
const noteDraft = ref('')
const showAllItems = ref(false)
const settingsOpen = ref(false)
const isFloating = ref(false)
const floatingPosition = ref<FloatingPosition | null>(null)
const panelRef = ref<HTMLElement | null>(null)
const isDragging = ref(false)

interface FloatingPosition {
  left: number
  top: number
}

let mounted = false
let loadSequence = 0
let lifecycleGeneration = 0
let watchGeneration = 0
let unlistenPlanUpdate: UnlistenFn | null = null
let animationTimer: ReturnType<typeof setTimeout> | null = null
let previousStatuses = new Map<string, PlanStatus>()
let listenerSetupPromise: Promise<boolean> | null = null
let watchSetupQueue: Promise<void> = Promise.resolve()

const items = computed(() => snapshot.value?.items ?? [])
const completed = computed(() => snapshot.value?.summary.completed ?? 0)
const total = computed(() => snapshot.value?.summary.total ?? 0)
const allCompleted = computed(() => snapshot.value?.summary.all_completed ?? false)
const progressPercent = computed(() => total.value === 0 ? 0 : Math.round((completed.value / total.value) * 100))
const realtimeError = computed(() => eventError.value || watchError.value)
const completedTailLimit = 4
// 中文说明：仅在展示层收起较早完成项，始终保留未完成项，不改变真实计划快照。
const hiddenCompletedCount = computed(() => Math.max(0, items.value.filter(item => item.status === 'completed').length - completedTailLimit))
const visibleItems = computed(() => {
  if (showAllItems.value || hiddenCompletedCount.value === 0)
    return items.value

  let hidden = hiddenCompletedCount.value
  return items.value.filter((item) => {
    if (item.status !== 'completed')
      return true
    if (hidden > 0) {
      hidden -= 1
      return false
    }
    return true
  })
})
const floatingStyle = computed(() => {
  if (!isFloating.value || !floatingPosition.value)
    return undefined
  return {
    left: `${floatingPosition.value.left}px`,
    top: `${floatingPosition.value.top}px`,
    right: 'auto',
    bottom: 'auto',
  }
})

function storageKey(kind: string, workspace = props.workspace): string {
  return `popup-plan-panel-${kind}:${encodeURIComponent(workspace)}`
}

function readStoredValue(key: string): string | null {
  try {
    return localStorage.getItem(key)
  }
  catch {
    return null
  }
}

function writeStoredValue(key: string, value: string): void {
  try {
    localStorage.setItem(key, value)
  }
  catch {
    // 中文说明：本地存储不可用时保留内存态，不阻断计划面板的核心读取。
  }
}

function removeStoredValue(key: string): void {
  try {
    localStorage.removeItem(key)
  }
  catch {
    // 中文说明：清理偏好失败不影响计划状态展示。
  }
}

function parseFloatingPosition(value: string | null): FloatingPosition | null {
  if (!value)
    return null
  try {
    const parsed = JSON.parse(value) as Partial<FloatingPosition>
    if (typeof parsed.left === 'number' && Number.isFinite(parsed.left) && typeof parsed.top === 'number' && Number.isFinite(parsed.top))
      return { left: parsed.left, top: parsed.top }
  }
  catch {
    // 中文说明：坐标损坏时回退到窗口右下角默认位置。
  }
  return null
}

let preferencesReady = false

function loadPanelPreferences(workspace = props.workspace): void {
  // 中文说明：备注、悬浮开关和坐标按工作区隔离，避免不同项目互相覆盖偏好。
  preferencesReady = false
  localNote.value = readStoredValue(storageKey('note', workspace)) ?? ''
  noteDraft.value = ''
  isFloating.value = readStoredValue(storageKey('floating', workspace)) === 'true'
  floatingPosition.value = parseFloatingPosition(readStoredValue(storageKey('position', workspace)))
  showAllItems.value = false
  settingsOpen.value = false
  preferencesReady = true
}

function persistFloatingPreferences(): void {
  if (!preferencesReady)
    return
  writeStoredValue(storageKey('floating'), String(isFloating.value))
  if (floatingPosition.value)
    writeStoredValue(storageKey('position'), JSON.stringify(floatingPosition.value))
  else
    removeStoredValue(storageKey('position'))
}

function saveLocalNote(): void {
  const nextNote = noteDraft.value.trim()
  if (!nextNote)
    return
  localNote.value = nextNote
  noteDraft.value = ''
  writeStoredValue(storageKey('note'), nextNote)
}

function clearLocalNote(): void {
  localNote.value = ''
  removeStoredValue(storageKey('note'))
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), Math.max(min, max))
}

function ensureDefaultFloatingPosition(): void {
  if (!isFloating.value || floatingPosition.value || !panelRef.value || typeof window === 'undefined')
    return
  const rect = panelRef.value.getBoundingClientRect()
  floatingPosition.value = {
    left: Math.max(12, window.innerWidth - rect.width - 16),
    top: Math.max(12, window.innerHeight - rect.height - 16),
  }
  persistFloatingPreferences()
}

let dragPointerId: number | null = null
let dragOffset = { x: 0, y: 0 }

function startDragging(event: PointerEvent): void {
  // 中文说明：仅允许从明确的拖动手柄开始，避免抢占标题折叠和正文选中文本。
  if (!isFloating.value || event.button !== 0 || !panelRef.value)
    return
  const rect = panelRef.value.getBoundingClientRect()
  dragPointerId = event.pointerId
  dragOffset = { x: event.clientX - rect.left, y: event.clientY - rect.top }
  isDragging.value = true
  panelRef.value.setPointerCapture?.(event.pointerId)
  event.preventDefault()
}

function handleDragging(event: PointerEvent): void {
  if (!isDragging.value || dragPointerId !== event.pointerId || !panelRef.value || typeof window === 'undefined')
    return
  const rect = panelRef.value.getBoundingClientRect()
  const margin = 12
  floatingPosition.value = {
    left: clamp(event.clientX - dragOffset.x, margin, window.innerWidth - rect.width - margin),
    top: clamp(event.clientY - dragOffset.y, margin, window.innerHeight - rect.height - margin),
  }
}

function stopDragging(event?: PointerEvent): void {
  if (!isDragging.value)
    return
  if (event && dragPointerId !== event.pointerId)
    return
  if (event)
    panelRef.value?.releasePointerCapture?.(event.pointerId)
  dragPointerId = null
  isDragging.value = false
  persistFloatingPreferences()
}

function statusIcon(status: PlanStatus): string {
  if (status === 'completed')
    return 'i-carbon-checkmark-filled text-green-600 dark:text-green-400'
  if (status === 'in_progress')
    return 'i-carbon-circle-dash text-primary-600 dark:text-primary-400'
  return 'i-carbon-radio-button text-on-surface-secondary'
}

function statusLabel(status: PlanStatus): string {
  if (status === 'completed')
    return '已完成'
  if (status === 'in_progress')
    return '进行中'
  return '待开始'
}

function statusLabelClass(status: PlanStatus): string {
  if (status === 'completed')
    return 'text-green-700 dark:text-green-300'
  if (status === 'in_progress')
    return 'text-primary-700 dark:text-primary-300'
  return 'text-on-surface-secondary'
}

function applySnapshot(nextSnapshot: PlanSnapshot) {
  const newlyCompleted = nextSnapshot.items.find(item =>
    item.status === 'completed'
    && previousStatuses.has(item.id)
    && previousStatuses.get(item.id) !== 'completed',
  )

  snapshot.value = nextSnapshot
  previousStatuses = new Map(nextSnapshot.items.map(item => [item.id, item.status]))

  if (newlyCompleted) {
    completedAnimationId.value = newlyCompleted.id
    if (animationTimer)
      clearTimeout(animationTimer)
    animationTimer = setTimeout(() => {
      completedAnimationId.value = ''
    }, 260)
  }
}

async function loadPlan(showLoading = false, workspace = props.workspace) {
  const sequence = ++loadSequence
  if (showLoading)
    loading.value = true
  readError.value = ''

  try {
    const nextSnapshot = await invoke<PlanSnapshot>('get_plan_snapshot', {
      workspace,
    })
    if (sequence === loadSequence)
      applySnapshot(nextSnapshot)
  }
  catch (error) {
    if (sequence === loadSequence)
      readError.value = String(error)
  }
  finally {
    if (sequence === loadSequence)
      loading.value = false
  }
}

function isCurrentWatch(generation: number): boolean {
  return mounted && generation === watchGeneration
}

function isCurrentLifecycle(generation: number): boolean {
  return mounted && generation === lifecycleGeneration
}

async function stopWorkspaceWatch() {
  try {
    await invoke('stop_plan_watch')
  }
  catch (error) {
    console.warn('停止计划文件监听失败：', error)
  }
}

async function startWorkspaceWatch(generation: number, workspace: string) {
  if (!isCurrentWatch(generation))
    return

  watchError.value = ''
  previousStatuses.clear()
  snapshot.value = null
  loading.value = true

  let started = false
  try {
    await invoke('start_plan_watch', { workspace })
    started = true
  }
  catch (error) {
    if (isCurrentWatch(generation))
      watchError.value = String(error)
  }

  if (!isCurrentWatch(generation)) {
    if (started)
      await stopWorkspaceWatch()
    return
  }

  // 中文说明：监听建立后再读取，覆盖监听启动前发生更新的竞态窗口。
  loadPlan(true, workspace)
}

function queueWorkspaceWatch(generation: number, workspace: string): Promise<void> {
  // 中文说明：串行建立 watcher，过期任务完成后先清理，再启动最新工作区。
  watchSetupQueue = watchSetupQueue.then(() => startWorkspaceWatch(generation, workspace))
  return watchSetupQueue
}

async function retry() {
  await restartWorkspaceWatch()
}

watch(isFloating, (enabled) => {
  if (!preferencesReady)
    return
  if (enabled)
    nextTick(ensureDefaultFloatingPosition)
  persistFloatingPreferences()
}, { flush: 'post' })

async function ensurePlanListener(): Promise<boolean> {
  if (unlistenPlanUpdate)
    return mounted
  if (listenerSetupPromise)
    return listenerSetupPromise

  const generation = lifecycleGeneration
  const setupPromise = (async () => {
    try {
      const unlisten = await listen('plan-updated', () => {
        if (mounted)
          loadPlan()
      })
      if (!isCurrentLifecycle(generation)) {
        unlisten()
        return false
      }
      unlistenPlanUpdate = unlisten
      eventError.value = ''
      return true
    }
    catch (error) {
      if (!isCurrentLifecycle(generation))
        return false
      eventError.value = String(error)
      return true
    }
  })()
  listenerSetupPromise = setupPromise
  try {
    return await setupPromise
  }
  finally {
    if (listenerSetupPromise === setupPromise)
      listenerSetupPromise = null
  }
}

async function restartWorkspaceWatch() {
  const generation = ++watchGeneration
  const workspace = props.workspace
  loadSequence += 1

  if (await ensurePlanListener()) {
    if (isCurrentWatch(generation))
      await queueWorkspaceWatch(generation, workspace)
  }
}

onMounted(async () => {
  mounted = true
  loadPanelPreferences()
  await restartWorkspaceWatch()
})

watch(() => props.workspace, async (workspace, previousWorkspace) => {
  if (mounted && workspace !== previousWorkspace) {
    loadPanelPreferences(workspace)
    await restartWorkspaceWatch()
  }
})

onUnmounted(() => {
  mounted = false
  lifecycleGeneration += 1
  watchGeneration += 1
  loadSequence += 1
  if (animationTimer)
    clearTimeout(animationTimer)
  stopDragging()
  unlistenPlanUpdate?.()
  unlistenPlanUpdate = null
  stopWorkspaceWatch()
})
</script>

<template>
  <!-- 中文说明：卡片只负责建立计划信息层级，读取与监听仍由原有逻辑维护。 -->
  <section
    ref="panelRef"
    class="plan-panel space-y-2 rounded-xl border border-gray-600/45 bg-container-secondary/70 p-3 shadow-sm"
    :class="[
      isFloating ? 'plan-panel--floating' : '',
      isDragging ? 'plan-panel--dragging' : '',
    ]"
    :style="floatingStyle"
    data-guide="plan-panel"
    aria-label="执行计划"
    @pointermove="handleDragging"
    @pointerup="stopDragging"
    @pointercancel="stopDragging"
    @keydown.esc="settingsOpen = false"
  >
    <div class="relative flex items-center justify-between gap-2">
      <button
        type="button"
        class="min-w-0 flex flex-1 items-center gap-2 rounded-md px-1.5 py-1 -ml-1.5 text-xs text-on-surface-secondary cursor-pointer select-none transition-colors hover:bg-container-tertiary/70 hover:text-on-surface focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary-500/40"
        :aria-expanded="!isCollapsed"
        @click="isCollapsed = !isCollapsed"
      >
        <div
          class="w-3 h-3 shrink-0 text-primary-500 transition-transform duration-200 motion-reduce:transition-none"
          :class="isCollapsed ? 'i-carbon-chevron-right' : 'i-carbon-chevron-down'"
        />
        <div class="i-carbon-list-checked w-3.5 h-3.5 shrink-0 text-teal-600 dark:text-teal-400" />
        <span class="truncate font-medium text-on-surface">执行计划</span>
        <span class="shrink-0 rounded-full border border-gray-500/45 px-1.5 py-0.5 text-[11px] leading-4 text-on-surface-secondary">{{ completed }}/{{ total }}</span>
      </button>

      <div class="flex shrink-0 items-center gap-0.5">
        <button
          v-if="isFloating"
          type="button"
          class="plan-drag-handle inline-flex h-6 w-6 items-center justify-center rounded text-on-surface-secondary transition-colors hover:bg-container-tertiary hover:text-on-surface focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary-500/40"
          aria-label="拖动执行计划"
          title="拖动执行计划"
          @pointerdown.stop="startDragging"
        >
          <div class="i-carbon-drag-vertical w-3.5 h-3.5" />
        </button>

        <button
          type="button"
          class="inline-flex h-6 w-6 items-center justify-center rounded text-on-surface-secondary transition-colors hover:bg-container-tertiary hover:text-on-surface focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary-500/40"
          :aria-expanded="settingsOpen"
          aria-label="执行计划设置"
          title="执行计划设置"
          @click.stop="settingsOpen = !settingsOpen"
        >
          <div class="i-carbon-settings w-3.5 h-3.5" />
        </button>

        <n-tooltip v-if="readError || realtimeError">
          <template #trigger>
            <n-button text size="tiny" class="shrink-0 opacity-70 hover:opacity-100" @click="retry">
              <template #icon>
                <div class="i-carbon-renew w-3.5 h-3.5" />
              </template>
            </n-button>
          </template>
          重新读取执行计划
        </n-tooltip>
      </div>

      <div v-if="settingsOpen" class="absolute right-0 top-full z-30 mt-1 w-56 rounded-lg border border-gray-600/60 bg-surface p-3 shadow-lg">
        <label class="flex cursor-pointer items-center gap-2 text-xs text-on-surface">
          <input v-model="isFloating" type="checkbox" class="h-3.5 w-3.5 accent-primary-500">
          <span>窗口内悬浮</span>
        </label>
        <p class="mt-1.5 text-[11px] leading-4 text-on-surface-secondary">
          开启后可使用标题旁的拖动手柄移动，位置会按工作区记忆。
        </p>
      </div>
    </div>

    <div v-if="!isCollapsed" class="space-y-2" aria-live="polite">
      <div v-if="loading" class="min-h-8 flex items-center gap-2 text-xs text-on-surface-secondary">
        <div class="i-carbon-circle-dash w-3.5 h-3.5 animate-spin motion-reduce:animate-none" />
        <span>正在读取计划...</span>
      </div>

      <div v-else-if="readError" class="rounded-lg border border-red-500/20 bg-red-500/8 p-2 text-xs text-red-700 dark:text-red-300">
        <div class="flex items-start gap-2">
          <div class="i-carbon-warning-alt w-3.5 h-3.5 mt-0.5 shrink-0" />
          <div class="min-w-0 break-words [overflow-wrap:anywhere]">
            <div class="font-medium">计划读取失败</div>
            <div class="mt-0.5 opacity-70">{{ readError }}</div>
          </div>
        </div>
      </div>

      <template v-else>
        <div v-if="realtimeError" class="flex items-start gap-2 rounded-lg border border-yellow-500/20 bg-yellow-500/8 p-2 text-xs text-yellow-700 dark:text-yellow-300">
          <div class="i-carbon-warning w-3.5 h-3.5 mt-0.5 shrink-0" />
          <div class="min-w-0 break-words [overflow-wrap:anywhere]">实时刷新不可用，可使用右侧按钮重新连接</div>
        </div>

        <div v-if="items.length === 0" class="min-h-8 flex items-center gap-2 text-xs text-on-surface-secondary">
          <div class="i-carbon-list-boxes w-3.5 h-3.5 shrink-0 opacity-70" />
          <span>暂无执行计划</span>
        </div>

        <template v-else>
          <!-- 中文说明：摘要固定展示完整快照进度，列表折叠只影响可见行数。 -->
          <div class="rounded-lg border border-gray-600/35 bg-surface/45 p-2">
            <div class="mb-1.5 flex items-center justify-between gap-2 text-[11px] text-on-surface-secondary">
              <span>完成度</span>
              <span class="font-medium text-on-surface">{{ completed }}/{{ total }} · {{ progressPercent }}%</span>
            </div>
            <div class="h-1.5 overflow-hidden rounded-full bg-container-tertiary" role="progressbar" aria-label="执行计划完成度" :aria-valuenow="progressPercent" aria-valuemin="0" aria-valuemax="100">
              <div
                class="h-full rounded-full bg-teal-600 transition-[width] duration-200 ease-out dark:bg-teal-400 motion-reduce:transition-none"
                :style="{ width: `${progressPercent}%` }"
              />
            </div>
          </div>

          <div class="space-y-1.5">
            <div class="flex items-center gap-2 rounded-lg border border-primary-500/20 bg-primary-500/5 px-2 py-1.5">
              <div class="i-carbon-edit w-3.5 h-3.5 shrink-0 text-primary-500" />
              <input
                v-model="noteDraft"
                type="text"
                class="min-w-0 flex-1 bg-transparent text-xs leading-5 text-on-surface outline-none placeholder:text-on-surface-muted"
                aria-label="添加执行计划本地备注"
                placeholder="添加本地备注，回车确认"
                @keydown.enter.prevent="saveLocalNote"
              >
            </div>
            <div v-if="localNote" class="flex items-start gap-2 rounded-lg border border-gray-600/35 bg-container-secondary px-2 py-1.5 text-xs text-on-surface-secondary">
              <div class="i-carbon-notebook w-3.5 h-3.5 mt-0.5 shrink-0 text-primary-500" />
              <span class="min-w-0 flex-1 break-words [overflow-wrap:anywhere]">{{ localNote }}</span>
              <button
                type="button"
                class="inline-flex h-5 w-5 shrink-0 items-center justify-center rounded text-on-surface-secondary transition-colors hover:bg-container-tertiary hover:text-on-surface focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary-500/40"
                aria-label="清除本地备注"
                title="清除本地备注"
                @click="clearLocalNote"
              >
                <div class="i-carbon-close w-3 h-3" />
              </button>
            </div>
          </div>

          <button
            v-if="hiddenCompletedCount > 0"
            type="button"
            class="inline-flex items-center gap-1 rounded-md px-1.5 py-1 text-[11px] text-on-surface-secondary transition-colors hover:bg-container-tertiary hover:text-on-surface focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary-500/40"
            :aria-expanded="showAllItems"
            @click="showAllItems = !showAllItems"
          >
            <div class="w-3 h-3" :class="showAllItems ? 'i-carbon-chevron-up' : 'i-carbon-chevron-down'" />
            <span>{{ showAllItems ? '收起较早完成项' : `显示此前 ${hiddenCompletedCount} 个已完成项` }}</span>
          </button>

          <div class="plan-list-scroll max-h-60 overflow-y-auto pr-1 scrollbar-thin">
            <ol class="space-y-1">
              <li
                v-for="item in visibleItems"
                :key="item.id"
                class="plan-item min-w-0 flex items-start gap-2 rounded-lg border border-transparent px-2 py-1.5 transition-colors duration-150 hover:bg-container-secondary"
                :class="[
                  item.status === 'completed' ? 'opacity-75' : '',
                  item.status === 'in_progress' ? 'border-primary-500/25 bg-primary-500/8' : '',
                  completedAnimationId === item.id ? 'plan-item-completed-now' : '',
                ]"
              >
                <div
                  class="plan-status-icon w-3.5 h-3.5 mt-0.5 shrink-0"
                  :class="statusIcon(item.status)"
                />
                <div class="min-w-0 flex-1 flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
                  <span
                    class="min-w-0 text-xs leading-5 text-on-surface break-words [overflow-wrap:anywhere]"
                    :class="item.status === 'completed' ? 'line-through decoration-gray-500/60' : ''"
                  >
                    {{ item.text }}
                  </span>
                  <span class="shrink-0 text-[11px] leading-4" :class="statusLabelClass(item.status)">
                    {{ statusLabel(item.status) }}
                  </span>
                </div>
              </li>
            </ol>
          </div>

          <div v-if="allCompleted" class="flex items-center gap-2 rounded-lg border border-green-500/20 bg-green-500/8 px-2 py-1.5 text-xs text-green-700 dark:text-green-300">
            <div class="i-carbon-checkmark-outline w-3.5 h-3.5 shrink-0" />
            <span>计划已全部完成</span>
          </div>
        </template>
      </template>
    </div>
  </section>
</template>

<style scoped>
.plan-panel--floating {
  position: fixed;
  right: 1rem;
  bottom: 1rem;
  z-index: 40;
  width: min(360px, calc(100vw - 1.5rem));
  max-width: calc(100vw - 1.5rem);
  max-height: calc(100vh - 1.5rem);
  box-shadow: 0 14px 36px rgba(0, 0, 0, 0.24);
}

.plan-panel--dragging {
  user-select: none;
}

.plan-drag-handle {
  touch-action: none;
  cursor: grab;
}

.plan-panel--dragging .plan-drag-handle {
  cursor: grabbing;
}

@keyframes plan-completed {
  0% { transform: scale(0.8); opacity: 0.4; }
  60% { transform: scale(1.15); opacity: 1; }
  100% { transform: scale(1); opacity: 1; }
}

.plan-item-completed-now .plan-status-icon {
  animation: plan-completed 220ms ease-out;
}

@media (prefers-reduced-motion: reduce) {
  .plan-item-completed-now .plan-status-icon {
    animation: none;
  }
}
</style>
