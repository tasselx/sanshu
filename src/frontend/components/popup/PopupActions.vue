<script setup lang="ts">
import type { McpRequest } from '../../types/popup'
import { computed, onMounted } from 'vue'
import { useShortcuts } from '../../composables/useShortcuts'

interface Props {
  request: McpRequest | null
  loading?: boolean
  submitting?: boolean
  canSubmit?: boolean
  canEnhance?: boolean
  connectionStatus?: string
  continueReplyEnabled?: boolean
  inputStatusText?: string
  enhanceEnabled?: boolean
  // 中文注释：已启用且可用于增强的 MCP 工具名称列表
  enhanceToolNames?: string[]
  // 中文注释：保活已等待时长（mm:ss），为空则不展示保活指示
  keepAliveText?: string
}

interface Emits {
  submit: []
  continue: []
  enhance: []
  openMcpToolsTab: []
}

const props = withDefaults(defineProps<Props>(), {
  loading: false,
  submitting: false,
  canSubmit: false,
  canEnhance: false,
  connectionStatus: '已连接',
  continueReplyEnabled: true,
  inputStatusText: '',
  enhanceEnabled: false,
  enhanceToolNames: () => [],
  keepAliveText: '',
})

const emit = defineEmits<Emits>()

// 使用自定义快捷键系统
const {
  quickSubmitShortcutText,
  enhanceShortcutText,
  continueShortcutText,
  useQuickSubmitShortcut,
  useEnhanceShortcut,
  useContinueShortcut,
  loadShortcutConfig,
} = useShortcuts()

const shortcutText = quickSubmitShortcutText

// 中文注释：增强按钮的工具辅助提示文本
const enhanceToolHint = computed(() => {
  if (!props.enhanceToolNames || props.enhanceToolNames.length === 0) return ''
  return `增强时将引导 AI 使用 ${props.enhanceToolNames.join('、')} 辅助`
})

const statusText = computed(() => {
  // 如果可以提交，直接显示快捷键提示
  if (props.canSubmit) {
    return shortcutText.value
  }

  // 如果有输入状态文本且不是默认状态，显示输入状态
  if (props.inputStatusText && props.inputStatusText !== '等待输入...') {
    return props.inputStatusText
  }

  // 根据请求类型显示不同的提示
  if (props.request?.predefined_options) {
    return '选择选项或输入文本'
  }
  return '请输入内容'
})

// 处理快捷键
useQuickSubmitShortcut(() => {
  if (props.canSubmit && !props.submitting) {
    handleSubmit()
  }
})

useEnhanceShortcut(() => {
  if (!props.submitting && props.canEnhance) {
    handleEnhance()
  }
})

useContinueShortcut(() => {
  if (!props.submitting) {
    handleContinue()
  }
})

function handleSubmit() {
  if (props.canSubmit && !props.submitting) {
    emit('submit')
  }
}

function handleContinue() {
  if (!props.submitting) {
    emit('continue')
  }
}

function handleEnhance() {
  if (!props.submitting) {
    if (props.enhanceEnabled && props.canEnhance) {
      emit('enhance')
    }
    else {
      emit('openMcpToolsTab')
    }
  }
}

// 组件挂载时加载快捷键配置
onMounted(() => {
  loadShortcutConfig()
})
</script>

<template>
  <div class="px-4 py-3 bg-gray-100 min-h-[60px] select-none">
    <div v-if="!loading" class="flex justify-between items-center">
      <!-- 左侧状态信息 -->
      <div class="flex items-center">
        <div class="flex items-center gap-2 text-xs text-gray-600">
          <div class="w-2 h-2 rounded-full bg-primary-500" />
          <span class="font-medium">{{ connectionStatus }}</span>
          <span class="opacity-60">|</span>
          <span class="opacity-60">{{ statusText }}</span>
          <!-- 防超时保活指示：绿色呼吸点 + 已等待时长，告知用户可慢慢想、输入不会丢失 -->
          <template v-if="keepAliveText">
            <span class="opacity-60">|</span>
            <n-tooltip trigger="hover" placement="top">
              <template #trigger>
                <span class="flex items-center gap-1.5 opacity-70">
                  <span class="keepalive-dot" />
                  <span>防超时保活中 · 已等待 {{ keepAliveText }}</span>
                </span>
              </template>
              正通过心跳 + 短调用重连保活，慢慢想，输入不会丢失
            </n-tooltip>
          </template>
        </div>
      </div>

      <!-- 右侧操作按钮 -->
      <div class="flex items-center" data-guide="popup-actions">
        <n-space size="small">
          <!-- 增强按钮 / 启用 CTA -->
          <n-tooltip v-if="enhanceEnabled" trigger="hover" placement="top">
            <template #trigger>
              <n-button
                :disabled="!canEnhance || submitting"
                size="medium"
                type="info"
                data-guide="enhance-button"
                @click="handleEnhance"
              >
                <template #icon>
                  <div class="i-carbon-magic-wand w-4 h-4" />
                </template>
                本地增强
              </n-button>
            </template>
            <div>
              <div>{{ canEnhance ? enhanceShortcutText : '请先输入要增强的文本' }}</div>
              <div v-if="enhanceToolHint && canEnhance" class="mt-1 text-xs opacity-75">
                {{ enhanceToolHint }}
              </div>
            </div>
          </n-tooltip>
          <n-tooltip v-else trigger="hover" placement="top">
            <template #trigger>
              <n-button
                :disabled="submitting"
                size="medium"
                type="warning"
                data-guide="enhance-cta"
                @click="handleEnhance"
              >
                <template #icon>
                  <div class="i-carbon-launch w-4 h-4" />
                </template>
                启用增强
              </n-button>
            </template>
            前往 MCP 工具启用提示词增强
          </n-tooltip>

          <!-- 继续按钮 -->
          <n-tooltip v-if="continueReplyEnabled" trigger="hover" placement="top">
            <template #trigger>
              <n-button
                :disabled="submitting"
                :loading="submitting"
                size="medium"
                type="default"
                data-guide="continue-button"
                @click="handleContinue"
              >
                <template #icon>
                  <div class="i-carbon-play w-4 h-4" />
                </template>
                继续
              </n-button>
            </template>
            {{ continueShortcutText }}
          </n-tooltip>

          <!-- 发送按钮 -->
          <n-tooltip trigger="hover" placement="top">
            <template #trigger>
              <n-button
                type="primary"
                :disabled="!canSubmit || submitting"
                :loading="submitting"
                size="medium"
                data-guide="submit-button"
                @click="handleSubmit"
              >
                <template #icon>
                  <div v-if="!submitting" class="i-carbon-send w-4 h-4" />
                </template>
                {{ submitting ? '发送中...' : '发送' }}
              </n-button>
            </template>
            {{ shortcutText }}
          </n-tooltip>
        </n-space>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 保活状态指示点：绿色呼吸动画，低对比不抢眼 */
.keepalive-dot {
  width: 7px;
  height: 7px;
  border-radius: 9999px;
  background: #22c55e;
  box-shadow: 0 0 0 0 rgba(34, 197, 94, 0.45);
  animation: keepalive-breathe 1.8s ease-in-out infinite;
}

@keyframes keepalive-breathe {
  0%, 100% {
    opacity: 0.55;
    box-shadow: 0 0 0 0 rgba(34, 197, 94, 0.45);
  }
  50% {
    opacity: 1;
    box-shadow: 0 0 0 4px rgba(34, 197, 94, 0);
  }
}

/* 无障碍：尊重系统"减少动态效果"设置 */
@media (prefers-reduced-motion: reduce) {
  .keepalive-dot {
    animation: none;
    opacity: 0.9;
  }
}
</style>
