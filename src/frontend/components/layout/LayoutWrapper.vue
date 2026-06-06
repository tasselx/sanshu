<script setup lang="ts">
import MainLayout from './MainLayout.vue'

interface AppConfig {
  theme: string
  window: {
    alwaysOnTop: boolean
    width: number
    height: number
    fixed: boolean
  }
  audio: {
    enabled: boolean
    url: string
  }
  reply: {
    enabled: boolean
    prompt: string
  }
}

interface Props {
  appConfig: AppConfig
  activeTab?: string
  projectRootPath?: string | null
  autoOpenToolId?: string | null
  autoOpenToolRequestId?: number
}

interface Emits {
  'themeChange': [theme: string]
  'toggleAlwaysOnTop': []
  'toggleAudioNotification': []
  'updateAudioUrl': [url: string]
  'testAudio': []
  'stopAudio': []
  'testAudioError': [error: any]
  'updateWindowSize': [size: { width: number, height: number, fixed: boolean }]
  'configReloaded': []
  'update:activeTab': [tab: string]
  'mcpToolAutoOpened': [requestId: number]
}

defineProps<Props>()
defineEmits<Emits>()
</script>

<template>
  <MainLayout
    :current-theme="appConfig.theme"
    :always-on-top="appConfig.window.alwaysOnTop"
    :audio-notification-enabled="appConfig.audio.enabled"
    :audio-url="appConfig.audio.url"
    :window-width="appConfig.window.width"
    :window-height="appConfig.window.height"
    :fixed-window-size="appConfig.window.fixed"
    :active-tab="activeTab"
    :project-root-path="projectRootPath || undefined"
    :auto-open-tool-id="autoOpenToolId || undefined"
    :auto-open-tool-request-id="autoOpenToolRequestId"
    @theme-change="$emit('themeChange', $event)"
    @toggle-always-on-top="$emit('toggleAlwaysOnTop')"
    @toggle-audio-notification="$emit('toggleAudioNotification')"
    @update-audio-url="$emit('updateAudioUrl', $event)"
    @test-audio="$emit('testAudio')"
    @stop-audio="$emit('stopAudio')"
    @test-audio-error="$emit('testAudioError', $event)"
    @update-window-size="$emit('updateWindowSize', $event)"
    @config-reloaded="$emit('configReloaded')"
    @update:active-tab="$emit('update:activeTab', $event)"
    @mcp-tool-auto-opened="$emit('mcpToolAutoOpened', $event)"
  />
</template>
