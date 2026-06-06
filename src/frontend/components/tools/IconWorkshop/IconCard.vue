<script setup lang="ts">
import type { IconItem } from '../../../types/icon'
/**
 * 图标卡片组件
 * 展示单个图标，支持选择、复制操作
 */
import { computed, ref } from 'vue'

// Props
interface Props {
  icon: IconItem
  selected?: boolean
}
const props = withDefaults(defineProps<Props>(), {
  selected: false,
})

// Emits
const emit = defineEmits<{
  toggle: []
  copy: []
  dblclick: []
  contextmenu: [event: MouseEvent]
}>()

// 悬停状态
const isHovered = ref(false)

// 计算属性
const displayName = computed(() => {
  const name = props.icon.name
  // 限制显示长度
  return name.length > 12 ? `${name.slice(0, 10)}...` : name
})

// SVG 内容（用于预览）
const svgContent = computed(() => {
  if (!props.icon.svgContent)
    return null

  // 移除尺寸限制，让 SVG 自适应父容器
  // 1. 移除 style 属性（解决 width: 1em 问题）
  // 2. 将 width/height 属性设为 100%
  return props.icon.svgContent
    .replace(/\s*style="[^"]*"/g, '')
    .replace(/\s*width="[^"]*"/g, ' width="100%"')
    .replace(/\s*height="[^"]*"/g, ' height="100%"')
})

// 点击卡片切换选择
function handleClick() {
  emit('toggle')
}

// 双击打开编辑器
function handleDblClick() {
  emit('dblclick')
}

// 右键菜单
function handleContextMenu(e: MouseEvent) {
  e.preventDefault()
  emit('contextmenu', e)
}

// 复制图标
function handleCopy(e: Event) {
  e.stopPropagation()
  emit('copy')
}
</script>

<template>
  <div
    class="icon-card"
    :class="{
      'icon-card--selected': selected,
      'icon-card--hovered': isHovered,
    }"
    @click="handleClick"
    @dblclick="handleDblClick"
    @contextmenu="handleContextMenu"
    @mouseenter="isHovered = true"
    @mouseleave="isHovered = false"
  >
    <!-- 选中标记 -->
    <transition name="scale">
      <div v-if="selected" class="selected-badge">
        <div class="i-carbon-checkmark text-white text-xs" />
      </div>
    </transition>

    <!-- 图标预览 -->
    <div class="icon-preview">
      <!-- SVG 内容直接渲染 -->
      <div
        v-if="svgContent"
        class="svg-container"
        v-html="svgContent"
      />
      <!-- 备用：使用字体类名 -->
      <div
        v-else-if="icon.fontClass"
        class="font-icon"
        :class="icon.fontClass"
      />
      <!-- 兜底：显示 ID -->
      <div v-else class="icon-placeholder">
        <div class="i-carbon-image text-2xl opacity-30" />
      </div>
    </div>

    <!-- 图标名称 -->
    <div class="icon-name" :title="icon.name">
      {{ displayName }}
    </div>

    <!-- 悬停操作 -->
    <transition name="fade">
      <div v-if="isHovered" class="icon-actions">
        <n-tooltip>
          <template #trigger>
            <n-button
              size="tiny"
              circle
              quaternary
              @click="handleCopy"
            >
              <template #icon>
                <div class="i-carbon-copy" />
              </template>
            </n-button>
          </template>
          复制 SVG
        </n-tooltip>
      </div>
    </transition>
  </div>
</template>

<style scoped>
.icon-card {
  position: relative;
  aspect-ratio: 1;
  padding: 12px;
  border-radius: 12px;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  cursor: pointer;
  transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
  background: var(--color-container, rgba(255, 255, 255, 0.8));
  border: 1px solid var(--color-border, rgba(128, 128, 128, 0.2));
}

:root.dark .icon-card {
  background: rgba(24, 24, 28, 0.9);
  border-color: rgba(255, 255, 255, 0.08);
}

/* 悬停效果 */
.icon-card--hovered {
  transform: scale(1.05);
  box-shadow: 0 8px 25px -5px rgba(139, 92, 246, 0.15);
  border-color: rgba(139, 92, 246, 0.3);
}

:root.dark .icon-card--hovered {
  box-shadow: 0 8px 25px -5px rgba(139, 92, 246, 0.25);
  border-color: rgba(139, 92, 246, 0.4);
}

/* 选中效果 */
.icon-card--selected {
  border-color: rgba(139, 92, 246, 0.6);
  background: rgba(139, 92, 246, 0.05);
}

:root.dark .icon-card--selected {
  border-color: rgba(139, 92, 246, 0.5);
  background: rgba(139, 92, 246, 0.1);
}

/* 选中标记 */
.selected-badge {
  position: absolute;
  top: 6px;
  right: 6px;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: linear-gradient(135deg, #8b5cf6, #a78bfa);
  display: flex;
  align-items: center;
  justify-content: center;
  box-shadow: 0 2px 6px rgba(139, 92, 246, 0.4);
}

/* 图标预览 */
.icon-preview {
  width: 40px;
  height: 40px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--color-on-surface, #111827);
  overflow: hidden; /* 防止 SVG 溢出 */
  flex-shrink: 0;
}

:root.dark .icon-preview {
  color: #e5e7eb;
}

.svg-container {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
}

.svg-container :deep(svg) {
  width: 100%;
  height: 100%;
  max-width: 40px;
  max-height: 40px;
  object-fit: contain;
}

.font-icon {
  font-size: 32px;
}

.icon-placeholder {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
}

/* 图标名称 */
.icon-name {
  font-size: 11px;
  color: var(--color-on-surface-secondary, #6b7280);
  text-align: center;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

:root.dark .icon-name {
  color: #9ca3af;
}

/* 悬停操作 */
.icon-actions {
  position: absolute;
  bottom: 6px;
  right: 6px;
}

/* ========== 过渡动画 ========== */
.scale-enter-active,
.scale-leave-active {
  transition: all 0.2s ease;
}

.scale-enter-from,
.scale-leave-to {
  opacity: 0;
  transform: scale(0.6);
}

.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.2s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
