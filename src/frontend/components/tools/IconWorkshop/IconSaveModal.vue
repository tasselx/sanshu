<script setup lang="ts">
/**
 * 图标保存模态框组件 - 重构版
 * 提供沉浸式的保存配置和预览体验
 */
import type { IconFormat, IconItem, IconSaveRequest } from '../../../types/icon'
import { invoke } from '@tauri-apps/api/core'
import { computed, ref, watch } from 'vue'

// Props
interface Props {
  show: boolean
  icons: IconItem[]
  defaultPath?: string
}

const props = withDefaults(defineProps<Props>(), {
  defaultPath: 'assets/icons',
})

// Emits
const emit = defineEmits<{
  'update:show': [value: boolean]
  'save': [request: IconSaveRequest]
}>()

// 本地状态
const savePath = ref(props.defaultPath)
const format = ref<IconFormat>('svg')
const saving = ref(false)

// 格式选项配置
const formatOptions = [
  {
    label: 'SVG 矢量',
    value: 'svg',
    desc: '保留原始矢量数据，可无限缩放',
    icon: 'i-carbon-vector-pen',
  },
  {
    label: 'PNG 位图',
    value: 'png',
    desc: '标清位图，兼容性好',
    icon: 'i-carbon-image',
  },
  {
    label: '双格式',
    value: 'both',
    desc: '同时保存 SVG 和 PNG 版本',
    icon: 'i-carbon-copy-file',
  },
] as const

// 监听默认路径变化
watch(() => props.defaultPath, (newPath) => {
  if (newPath) {
    savePath.value = newPath
  }
})

// 计算属性
const dialogVisible = computed({
  get: () => props.show,
  set: (value: boolean) => emit('update:show', value),
})

const iconCount = computed(() => props.icons.length)

// 清理 SVG 内容用于预览
function processSvg(content?: string) {
  if (!content)
    return null
  return content
    .replace(/\s*style="[^"]*"/g, '')
    .replace(/\s*width="[^"]*"/g, ' width="100%"')
    .replace(/\s*height="[^"]*"/g, ' height="100%"')
}

// 选择目录
async function selectDirectory() {
  try {
    const result = await invoke<string | null>('select_icon_save_directory', {
      defaultPath: savePath.value,
    })
    if (result) {
      savePath.value = result
    }
  }
  catch (e) {
    console.error('选择目录失败:', e)
  }
}

// 执行保存
async function handleSave() {
  if (!savePath.value.trim())
    return

  saving.value = true
  try {
    emit('save', {
      icons: props.icons,
      savePath: savePath.value,
      format: format.value,
    })
  }
  finally {
    saving.value = false
  }
}

// 取消
function handleCancel() {
  dialogVisible.value = false
}
</script>

<template>
  <n-modal
    v-model:show="dialogVisible"
    :mask-closable="!saving"
    :close-on-esc="!saving"
    transform-origin="center"
    class="custom-modal"
  >
    <div class="w-[800px] max-w-[95vw] bg-white dark:bg-[#1a1a1a] rounded-xl overflow-hidden shadow-2xl flex flex-col md:flex-row h-[600px] max-h-[90vh]">
      <!-- 左侧：配置面板 -->
      <div class="w-full md:w-[320px] bg-slate-50 dark:bg-[#1f1f23] p-6 flex flex-col border-r border-gray-100 dark:border-white/5">
        <div class="mb-6">
          <h2 class="text-xl font-bold text-slate-800 dark:text-slate-100 flex items-center gap-2">
            <div class="i-carbon-save text-indigo-500" />
            保存图标
          </h2>
          <p class="text-sm text-slate-500 dark:text-slate-400 mt-1">
            配置导出选项和目标路径
          </p>
        </div>

        <div class="flex-1 flex flex-col gap-6 overflow-y-auto pr-2">
          <!-- 路径选择 -->
          <div class="flex flex-col gap-2">
            <label class="text-xs font-semibold uppercase tracking-wider text-slate-400">保存路径</label>
            <div class="flex gap-2">
              <n-input
                v-model:value="savePath"
                placeholder="选择目录..."
                size="medium"
                class="flex-1"
              >
                <template #prefix>
                  <div class="i-carbon-folder text-slate-400" />
                </template>
              </n-input>
              <n-button secondary type="primary" class="!px-3" @click="selectDirectory">
                <div class="i-carbon-folder-open text-lg" />
              </n-button>
            </div>
          </div>

          <!-- 格式选择卡片 -->
          <div class="flex flex-col gap-3">
            <label class="text-xs font-semibold uppercase tracking-wider text-slate-400">导出格式</label>
            <div class="flex flex-col gap-2">
              <div
                v-for="opt in formatOptions"
                :key="opt.value"
                class="relative px-4 py-3 rounded-lg border-2 cursor-pointer transition-all duration-200 group flex items-center gap-3"
                :class="[
                  format === opt.value
                    ? 'border-indigo-500 bg-indigo-50/50 dark:bg-indigo-500/10'
                    : 'border-transparent bg-white dark:bg-white/5 hover:border-slate-200 dark:hover:border-white/10',
                ]"
                @click="format = opt.value as IconFormat"
              >
                <!-- 选中标记 -->
                <div v-if="format === opt.value" class="absolute right-2 top-2 text-indigo-500">
                  <div class="i-carbon-checkmark-filled text-lg" />
                </div>

                <div
                  class="w-10 h-10 rounded-full flex items-center justify-center text-xl transition-colors"
                  :class="format === opt.value ? 'bg-indigo-100 text-indigo-600 dark:bg-indigo-500/20 dark:text-indigo-400' : 'bg-slate-100 text-slate-400 dark:bg-white/10'"
                >
                  <div :class="opt.icon" />
                </div>

                <div class="flex-1">
                  <div class="font-medium text-slate-700 dark:text-slate-200" :class="{ 'text-indigo-600 dark:text-indigo-400': format === opt.value }">
                    {{ opt.label }}
                  </div>
                  <div class="text-xs text-slate-400 leading-tight mt-0.5">
                    {{ opt.desc }}
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- 底部按钮 -->
        <div class="mt-6 pt-4 border-t border-gray-100 dark:border-white/5 flex flex-col gap-3">
          <n-button
            type="primary"
            size="large"
            block
            :loading="saving"
            :disabled="!savePath.trim()"
            class="!h-12 !text-base !font-medium shadow-lg shadow-indigo-500/20"
            @click="handleSave"
          >
            <template #icon>
              <div class="i-carbon-download" />
            </template>
            确认保存 ({{ iconCount }})
          </n-button>

          <n-button quaternary block class="text-slate-500 hover:text-slate-700 dark:hover:text-slate-300" @click="handleCancel">
            取消
          </n-button>
        </div>
      </div>

      <!-- 右侧：预览面板 -->
      <div class="flex-1 bg-slate-100/50 dark:bg-[#121214] flex flex-col relative overflow-hidden">
        <!-- 装饰背景 -->
        <div class="absolute inset-0 pointer-events-none opacity-30 dark:opacity-10">
          <div class="absolute top-0 right-0 w-64 h-64 bg-indigo-500/10 rounded-full blur-3xl transform translate-x-1/2 -translate-y-1/2" />
          <div class="absolute bottom-0 left-0 w-48 h-48 bg-blue-500/10 rounded-full blur-3xl transform -translate-x-1/2 translate-y-1/2" />
        </div>

        <div class="p-6 pb-2 relative z-10 flex justify-between items-end border-b border-gray-100/50 dark:border-white/5">
          <div>
            <h3 class="text-lg font-semibold text-slate-700 dark:text-slate-200">
              预览清单
            </h3>
            <p class="text-sm text-slate-400">
              即将保存以下 {{ iconCount }} 个图标
            </p>
          </div>
          <div class="text-xs font-mono text-indigo-500 bg-indigo-50 dark:bg-indigo-500/10 px-2 py-1 rounded">
            SVG Render
          </div>
        </div>

        <div class="flex-1 overflow-y-auto p-6 relative z-10 custom-scrollbar">
          <div class="grid grid-cols-3 sm:grid-cols-4 lg:grid-cols-4 gap-4">
            <div
              v-for="icon in icons"
              :key="icon.id"
              class="group relative aspect-square rounded-xl bg-white dark:bg-[#1a1a1a] border border-gray-100 dark:border-white/5 shadow-sm hover:shadow-md hover:border-indigo-500/30 transition-all duration-300 flex flex-col items-center justify-center p-4"
            >
              <!-- 图标 -->
              <div class="flex-1 w-full flex items-center justify-center text-slate-700 dark:text-slate-200 group-hover:text-indigo-500 transition-colors">
                <div
                  v-if="icon.svgContent"
                  class="w-8 h-8 md:w-10 md:h-10 transition-transform duration-300 group-hover:scale-110"
                  v-html="processSvg(icon.svgContent)"
                />
                <div v-else class="i-carbon-image text-4xl opacity-20" />
              </div>

              <!-- 名称 -->
              <div class="w-full text-center mt-3">
                <div class="text-xs text-slate-400 group-hover:text-slate-600 dark:group-hover:text-slate-300 truncate transition-colors font-medium">
                  {{ icon.name }}
                </div>
              </div>

              <!-- 悬停时的角标 -->
              <div class="absolute top-2 right-2 opacity-0 group-hover:opacity-100 transition-opacity">
                <div class="w-2 h-2 rounded-full bg-indigo-500 shadow-sm" />
              </div>
            </div>
          </div>

          <!-- 空状态修正 -->
          <div v-if="icons.length === 0" class="h-full flex flex-col items-center justify-center text-slate-300 dark:text-slate-600">
            <div class="i-carbon-select-window text-6xl mb-4" />
            <p>未选择图标</p>
          </div>
        </div>

        <!-- 底部渐变遮罩 -->
        <div class="absolute bottom-0 left-0 right-0 h-12 bg-gradient-to-t from-slate-100 dark:from-[#121214] to-transparent pointer-events-none z-20" />
      </div>
    </div>
  </n-modal>
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
