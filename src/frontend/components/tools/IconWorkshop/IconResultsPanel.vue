<script setup lang="ts">
import type { IconItem } from '../../../types/icon'
/**
 * 图标结果面板
 * 负责瀑布流渲染、加载更多与滚动触发
 */
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import IconCard from './IconCard.vue'
import IconCardSkeleton from './IconCardSkeleton.vue'

interface Props {
  icons: IconItem[]
  selectedIds: Set<number>
  loading: boolean
  hasMore: boolean
  currentPage: number
  pageSize: number
  total: number
  isEmpty: boolean
  showEmptyState: boolean
}

const props = defineProps<Props>()

const emit = defineEmits<{
  'toggle': [iconId: number]
  'copy': [icon: IconItem]
  'dblclick': [icon: IconItem]
  'contextmenu': [icon: IconItem, event: MouseEvent]
  'load-more': []
  'jump': [page: number]
}>()

const hasResults = computed(() => props.icons.length > 0)
const maxPage = computed(() => Math.max(1, Math.ceil(props.total / props.pageSize)))
const scrollContainer = ref<HTMLElement | null>(null)
const loadMoreTrigger = ref<HTMLElement | null>(null)
const isAutoLoading = ref(false)
const jumpPage = ref<number | null>(null)
let observer: IntersectionObserver | null = null

function handleLoadMore() {
  if (props.loading || !props.hasMore)
    return
  emit('load-more')
}

function handleJump() {
  if (!jumpPage.value)
    return
  const target = Math.min(Math.max(1, Math.floor(jumpPage.value)), maxPage.value)
  emit('jump', target)
}

watch(() => props.loading, (value) => {
  if (!value)
    isAutoLoading.value = false
})

watch(
  [loadMoreTrigger, scrollContainer, () => props.hasMore],
  ([trigger, container, hasMore]) => {
    if (observer) {
      observer.disconnect()
      observer = null
    }
    if (!trigger || !container || !hasMore)
      return

    // 底部进入视口时自动加载，增强瀑布流体验
    observer = new IntersectionObserver((entries) => {
      if (!entries.some(entry => entry.isIntersecting))
        return
      if (isAutoLoading.value || props.loading || !props.hasMore)
        return
      isAutoLoading.value = true
      emit('load-more')
    }, {
      root: container,
      rootMargin: '0px 0px 240px 0px',
      threshold: 0.1,
    })
    observer.observe(trigger)
  },
  { immediate: true },
)

onBeforeUnmount(() => {
  if (observer)
    observer.disconnect()
})
</script>

<template>
  <div ref="scrollContainer" class="flex-1 overflow-y-auto min-h-0 pr-2 custom-scrollbar relative">
    <!-- 初始骨架屏 -->
    <div v-if="loading && !hasResults" class="columns-4 sm:columns-5 md:columns-6 lg:columns-8 gap-3">
      <div v-for="i in 32" :key="`skeleton-${i}`" class="mb-3 break-inside-avoid">
        <IconCardSkeleton />
      </div>
    </div>

    <template v-else-if="hasResults">
      <transition-group
        tag="div"
        class="columns-4 sm:columns-5 md:columns-6 lg:columns-8 gap-3"
        enter-active-class="transition-all duration-300 ease-out"
        enter-from-class="opacity-0 translate-y-2"
        enter-to-class="opacity-100 translate-y-0"
        leave-active-class="transition-all duration-200 ease-in"
        leave-from-class="opacity-100"
        leave-to-class="opacity-0"
      >
        <div
          v-for="icon in icons"
          :key="icon.id"
          class="mb-3 break-inside-avoid"
        >
          <IconCard
            :icon="icon"
            :selected="selectedIds.has(icon.id)"
            @toggle="emit('toggle', icon.id)"
            @copy="emit('copy', icon)"
            @dblclick="emit('dblclick', icon)"
            @contextmenu="emit('contextmenu', icon, $event)"
          />
        </div>
      </transition-group>

      <!-- 自动加载哨兵 -->
      <div v-if="hasMore" ref="loadMoreTrigger" class="h-1 w-full" />

      <!-- 加载指示器 -->
      <transition
        enter-active-class="transition-all duration-300 ease-out"
        enter-from-class="opacity-0 -translate-y-1"
        enter-to-class="opacity-100 translate-y-0"
        leave-active-class="transition-all duration-200 ease-in"
        leave-from-class="opacity-100 translate-y-0"
        leave-to-class="opacity-0 -translate-y-1"
      >
        <div v-if="loading && hasMore" class="flex items-center justify-center py-4 text-xs text-slate-400 dark:text-slate-500">
          <div class="i-carbon-circle-dash animate-spin text-base" />
          <span class="ml-2">加载中...</span>
        </div>
      </transition>

      <!-- 手动加载按钮 -->
      <div v-if="hasMore && !loading" class="flex justify-center py-6">
        <n-button secondary size="large" @click="handleLoadMore">
          加载更多
        </n-button>
      </div>
    </template>

    <!-- 空状态 - 无搜索结果 -->
    <div v-else-if="isEmpty" class="h-full flex flex-col items-center justify-center text-slate-400 dark:text-slate-500">
      <div class="w-24 h-24 rounded-full bg-slate-100/80 dark:bg-white/5 flex items-center justify-center mb-4">
        <div class="i-carbon-search-locate text-4xl opacity-50" />
      </div>
      <p class="text-sm">
        未找到相关图标，请尝试其他关键词
      </p>
    </div>

    <!-- 空状态 - 初始 -->
    <div v-else-if="showEmptyState" class="h-full flex flex-col items-center justify-center text-slate-300 dark:text-slate-600">
      <div class="i-carbon-image text-8xl opacity-10 mb-6" />
      <p class="text-lg font-medium opacity-80 mb-2">
        搜索 Iconfont 图标库
      </p>
      <p class="text-sm opacity-50">
        输入关键词开始探索无限创意
      </p>
    </div>
  </div>

  <!-- 悬浮分页组件 -->
  <transition
    enter-active-class="transition-all duration-300 ease-out"
    enter-from-class="opacity-0 translate-y-4"
    enter-to-class="opacity-100 translate-y-0"
    leave-active-class="transition-all duration-200 ease-in"
    leave-from-class="opacity-100 translate-y-0"
    leave-to-class="opacity-0 translate-y-4"
  >
    <div
      v-if="hasResults"
      class="fixed bottom-4 left-1/2 transform -translate-x-1/2 z-40 flex items-center gap-3 px-4 py-2 rounded-full bg-white/90 dark:bg-[#1f1f23]/95 border border-gray-200 dark:border-white/10 shadow-xl backdrop-blur-sm"
    >
      <span class="text-xs text-gray-500 dark:text-gray-400 whitespace-nowrap">
        第 {{ currentPage }} 页
      </span>
      <span class="text-xs text-gray-400 dark:text-gray-500 whitespace-nowrap">
        / {{ maxPage }} 页
      </span>
      <div class="h-4 w-px bg-gray-200 dark:bg-white/10" />
      <span class="text-xs text-gray-500 dark:text-gray-400 whitespace-nowrap">
        共 {{ total }} 个
      </span>
      <div class="h-4 w-px bg-gray-200 dark:bg-white/10" />
      <div class="flex items-center gap-2">
        <n-input-number
          v-model:value="jumpPage"
          size="tiny"
          :min="1"
          :max="maxPage"
          :disabled="loading"
          class="w-20"
          placeholder="页码"
        />
        <n-button size="tiny" secondary :disabled="loading" @click="handleJump">
          跳转
        </n-button>
      </div>
      <n-button
        v-if="hasMore"
        size="tiny"
        type="primary"
        :loading="loading"
        @click="handleLoadMore"
      >
        加载更多
      </n-button>
      <span v-else class="text-xs text-gray-400 dark:text-gray-500">已全部加载</span>
    </div>
  </transition>
</template>

<style scoped>
/* 自定义滚动条 */
.custom-scrollbar::-webkit-scrollbar {
  width: 6px;
}
.custom-scrollbar::-webkit-scrollbar-track {
  background: transparent;
}
.custom-scrollbar::-webkit-scrollbar-thumb {
  background-color: rgba(156, 163, 175, 0.2);
  border-radius: 3px;
}
.custom-scrollbar::-webkit-scrollbar-thumb:hover {
  background-color: rgba(156, 163, 175, 0.4);
}
</style>
