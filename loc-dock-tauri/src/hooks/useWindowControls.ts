import { useState, useEffect, useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";

export function useWindowControls() {
  const [pinned, setPinned] = useState(true);
  const win = getCurrentWindow();

  useEffect(() => {
    const unlisten = win.onMoved(() => {
      setPinned(false);
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  const close = useCallback(() => win.close(), []);

  const snapToCorner = useCallback(async () => {
    await invoke("snap_to_corner");
    setPinned(true);
  }, []);

  return { pinned, close, snapToCorner };
}
