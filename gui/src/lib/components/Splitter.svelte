<script lang="ts">
  // Drag handle between two sibling panes. Controlled: the parent
  // owns the `size` value (px of the first pane), and `onResize`
  // fires on every move. Kept deliberately dumb so both the
  // columns- and stacked-layout callers can compose it the same
  // way without the splitter worrying about orientation-specific
  // layout concerns.

  interface Props {
    orientation: 'horizontal' | 'vertical';
    size: number;
    min: number;
    max: number;
    onResize: (size: number) => void;
  }

  let { orientation, size, min, max, onResize }: Props = $props();

  let dragging = $state(false);
  let anchor = 0;
  let startSize = 0;

  function onMouseDown(e: MouseEvent) {
    dragging = true;
    anchor = orientation === 'horizontal' ? e.clientX : e.clientY;
    startSize = size;
    // Suppress text selection and show the resize cursor globally
    // so dragging off a narrow handle doesn't lose the grab.
    document.body.style.userSelect = 'none';
    document.body.style.cursor = orientation === 'horizontal' ? 'col-resize' : 'row-resize';
    e.preventDefault();
  }

  function onMouseMove(e: MouseEvent) {
    if (!dragging) return;
    const current = orientation === 'horizontal' ? e.clientX : e.clientY;
    const delta = current - anchor;
    const next = Math.max(min, Math.min(max, startSize + delta));
    onResize(next);
  }

  function onMouseUp() {
    if (!dragging) return;
    dragging = false;
    document.body.style.userSelect = '';
    document.body.style.cursor = '';
  }

  // Keyboard nudge for accessibility: arrow keys move by 16 px
  // once the handle is focused.
  function onKeyDown(e: KeyboardEvent) {
    const step = e.shiftKey ? 48 : 16;
    let delta = 0;
    if (orientation === 'horizontal') {
      if (e.key === 'ArrowLeft') delta = -step;
      else if (e.key === 'ArrowRight') delta = step;
    } else {
      if (e.key === 'ArrowUp') delta = -step;
      else if (e.key === 'ArrowDown') delta = step;
    }
    if (delta !== 0) {
      e.preventDefault();
      onResize(Math.max(min, Math.min(max, size + delta)));
    }
  }
</script>

<svelte:window onmousemove={onMouseMove} onmouseup={onMouseUp} />

<div
  class="group shrink-0 bg-surface-2/40 transition-colors hover:bg-sky-500/40
    {dragging ? 'bg-sky-500/60' : ''}
    {orientation === 'horizontal'
    ? 'h-full w-[3px] cursor-col-resize'
    : 'h-[3px] w-full cursor-row-resize'}"
  role="separator"
  aria-orientation={orientation === 'horizontal' ? 'vertical' : 'horizontal'}
  aria-valuenow={size}
  aria-valuemin={min}
  aria-valuemax={max}
  tabindex="0"
  onmousedown={onMouseDown}
  onkeydown={onKeyDown}
></div>
