import { getCurrentWebview } from '@tauri-apps/api/webview';
import { onMounted, onUnmounted, ref } from 'vue';

/// Tauri's native drag-drop, not HTML5 drag events. With `dragDropEnabled`
/// (the default), the webview intercepts OS file drops before the browser
/// event model ever sees them — and hands us real filesystem paths,
/// which is exactly what our add_files command needs.
export function useDragDrop(onDrop: (paths: string[]) => void) {
  const dragging = ref(false);
  let unlisten: (() => void) | undefined;

  onMounted(async () => {
    unlisten = await getCurrentWebview().onDragDropEvent((event) => {
      switch (event.payload.type) {
        case 'enter':
        case 'over':
          dragging.value = true;
          break;
        case 'leave':
          dragging.value = false;
          break;
        case 'drop':
          dragging.value = false;
          if (event.payload.paths.length > 0) onDrop(event.payload.paths);
          break;
      }
    });
  });

  // The promise may resolve after unmount in theory; harmless here
  // because App lives for the whole app. The pattern matters, though:
  // every listener registered should have a matching unregister.
  onUnmounted(() => unlisten?.());

  return { dragging };
}
