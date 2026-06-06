<script setup lang="ts">
import { NSpace, NTag, useMessage } from 'naive-ui'
import { computed, onMounted } from 'vue'
import { useVersionCheck } from '../../composables/useVersionCheck'

const message = useMessage()
const { versionInfo, manualCheckUpdate, safeOpenUrl, lastCheckTime, isChecking, getVersionInfo } = useVersionCheck()

// 格式化最后检查时间
const formattedLastCheckTime = computed(() => {
  return lastCheckTime.value ? lastCheckTime.value.toLocaleString('zh-CN') : ''
})

// 安全打开GitHub链接
async function openGitHub() {
  try {
    await safeOpenUrl('https://github.com/yuaotian/sanshu')
    message.success('正在打开GitHub页面...')
  }
  catch (error) {
    const errorMsg = error instanceof Error ? error.message : '打开GitHub失败，请手动访问'
    if (errorMsg.includes('已复制到剪贴板')) {
      message.warning(errorMsg)
    }
    else {
      message.error(errorMsg)
    }
  }
}

// 安全打开GitHub Star页面
async function openGitHubStars() {
  try {
    await safeOpenUrl('https://github.com/yuaotian/sanshu/stargazers')
    message.success('正在打开Star页面...')
  }
  catch (error) {
    const errorMsg = error instanceof Error ? error.message : '打开Star页面失败，请手动访问'
    if (errorMsg.includes('已复制到剪贴板')) {
      message.warning(errorMsg)
    }
    else {
      message.error(errorMsg)
    }
  }
}

// 检查版本更新
async function checkVersion() {
  try {
    const info = await manualCheckUpdate()
    if (info?.hasUpdate) {
      message.info(`发现新版本 v${info.latest}！`)
    }
    else {
      message.success('当前已是最新版本')
    }
  }
  catch (error) {
    console.error('检查版本失败:', error)
    message.error('检查版本失败，请稍后重试')
  }
}

// 功能亮点配置 (高定美学配色)
const features = [
  {
    label: '智能交互',
    icon: 'i-carbon-chat',
    colorClass: '!bg-blue-50 !text-blue-600 !border-blue-200 dark:!bg-blue-900/30 dark:!text-blue-300 dark:!border-blue-700/50',
  },
  {
    label: '全局记忆',
    icon: 'i-carbon-data-base',
    colorClass: '!bg-violet-50 !text-violet-600 !border-violet-200 dark:!bg-violet-900/30 dark:!text-violet-300 dark:!border-violet-700/50',
  },
  {
    label: '语义搜索',
    icon: 'i-carbon-search',
    colorClass: '!bg-emerald-50 !text-emerald-600 !border-emerald-200 dark:!bg-emerald-900/30 dark:!text-emerald-300 dark:!border-emerald-700/50',
  },
  {
    label: '框架文档',
    icon: 'i-carbon-document',
    colorClass: '!bg-orange-50 !text-orange-600 !border-orange-200 dark:!bg-orange-900/30 dark:!text-orange-300 dark:!border-orange-700/50',
  },
  {
    label: 'UI/UX 设计',
    icon: 'i-carbon-paint-brush',
    colorClass: '!bg-pink-50 !text-pink-600 !border-pink-200 dark:!bg-pink-900/30 dark:!text-pink-300 dark:!border-pink-700/50',
  },
  {
    label: '图标工坊',
    icon: 'i-carbon-image',
    colorClass: '!bg-indigo-50 !text-indigo-600 !border-indigo-200 dark:!bg-indigo-900/30 dark:!text-indigo-300 dark:!border-indigo-700/50',
  },
]

// 组件挂载时初始化版本信息
onMounted(async () => {
  try {
    await getVersionInfo()
  }
  catch (error) {
    console.error('初始化版本信息失败:', error)
  }
})
</script>

<template>
  <n-card
    size="small"
    class="transition-all duration-200 hover:shadow-md"
  >
    <!-- 主要内容区域 -->
    <div class="flex items-center justify-between mb-2">
      <!-- 左侧：项目信息 -->
      <div class="flex items-center gap-3">
        <div class="w-8 h-8 rounded-lg bg-blue-100 dark:bg-blue-900 flex items-center justify-center">
          <div class="i-carbon-logo-github text-blue-600 dark:text-blue-400" />
        </div>
        <div>
          <h3 class="font-semibold text-gray-900 dark:text-white text-sm">
            三术 {{ versionInfo ? `v${versionInfo.current}` : 'v0.2.0' }}
          </h3>
          <p class="text-xs text-gray-500 dark:text-gray-400">
            智能代码审查工具，支持MCP协议集成
          </p>
        </div>
      </div>

      <!-- 右侧：版本检查区域 -->
      <div class="flex flex-col items-end gap-1">
        <n-button
          size="medium"
          secondary
          :loading="isChecking"
          @click="checkVersion"
        >
          <template #icon>
            <div class="i-carbon-renew text-green-600 dark:text-green-400" />
          </template>
          检查更新
        </n-button>

        <!-- 最后检查时间 -->
        <div
          v-if="formattedLastCheckTime"
          class="text-xs text-gray-400 dark:text-gray-500"
        >
          最后检查: {{ formattedLastCheckTime }}
        </div>
      </div>
    </div>

    <!-- 功能亮点标签云 -->
    <div class="py-3 border-b border-gray-100 dark:border-gray-700">
      <NSpace size="small" :wrap="true">
        <NTag
          v-for="feature in features"
          :key="feature.label"
          size="small"
          :bordered="true"
          round
          class="transition-colors duration-300"
          :class="feature.colorClass"
        >
          <template #icon>
            <div class="text-xs" :class="[feature.icon]" />
          </template>
          {{ feature.label }}
        </NTag>
      </NSpace>
    </div>

    <!-- 底部：GitHub区域 -->
    <div class="flex items-center justify-between border-t border-gray-100 dark:border-gray-700 pt-2">
      <div class="flex items-center gap-1">
        <n-button
          size="medium"
          type="primary"
          @click="openGitHub"
        >
          <template #icon>
            <div class="i-carbon-logo-github" />
          </template>
          GitHub
        </n-button>

        <n-button
          size="medium"
          secondary
          @click="openGitHubStars"
        >
          <template #icon>
            <div class="i-carbon-star text-yellow-500" />
          </template>
          Star
        </n-button>
      </div>

      <!-- 弱化的提示文字 -->
      <p class="text-xs text-gray-400 dark:text-gray-500">
        如果对您有帮助，请给我们一个 Star ⭐
      </p>
    </div>
  </n-card>
</template>
