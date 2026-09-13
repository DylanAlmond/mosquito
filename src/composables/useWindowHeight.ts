import { LogicalSize } from '@tauri-apps/api/dpi';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { onMounted, onUnmounted, type Ref } from 'vue';

const WINDOW_WIDTH = 400;

export function useWindowHeight(target: Ref<HTMLElement | null>) {
  let observer: ResizeObserver | undefined;
  let frame: number | undefined;
  let lastHeight: number | undefined;
  let disposed = false;

  function schedule(height: number) {
    if (frame !== undefined) cancelAnimationFrame(frame);

    frame = requestAnimationFrame(() => {
      frame = undefined;
      const nextHeight = Math.ceil(height);

      if (nextHeight <= 0 || nextHeight === lastHeight) return;

      lastHeight = nextHeight;      

      void getCurrentWindow()
        .setSize(new LogicalSize(WINDOW_WIDTH,  nextHeight))
        .catch(() => {
          if (!disposed) lastHeight = undefined;
        });
    });
  }

  onMounted(() => {
    const element = target.value;
    if (!element) return;

    observer = new ResizeObserver(([entry]) => {
      if (entry) schedule(entry.contentRect.height);
    });

    observer.observe(element);
    schedule(element.getBoundingClientRect().height);
  });

  onUnmounted(() => {
    disposed = true;
    observer?.disconnect();
    if (frame !== undefined) cancelAnimationFrame(frame);
  });
}
