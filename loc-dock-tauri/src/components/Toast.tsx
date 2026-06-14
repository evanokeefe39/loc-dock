import { useState, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";

interface ToastItem {
  id: number;
  message: string;
}

let nextId = 0;

export function ToastContainer() {
  const [toasts, setToasts] = useState<ToastItem[]>([]);

  useEffect(() => {
    const unlisten = listen<string>("status-update", (event) => {
      const id = nextId++;
      setToasts(prev => [...prev.slice(-2), { id, message: event.payload }]);
      setTimeout(() => {
        setToasts(prev => prev.filter(t => t.id !== id));
      }, 3000);
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  if (toasts.length === 0) return null;

  return (
    <div className="toast-container">
      {toasts.map(t => (
        <div key={t.id} className="toast-item">{t.message}</div>
      ))}
    </div>
  );
}
