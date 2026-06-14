import { useState } from "react";
import { X } from "lucide-react";

const DISMISS_KEY = "locdock_dismiss_apikey_banner";

interface Props {
  visible: boolean;
  onSettings: () => void;
}

export function NotificationBanner({ visible, onSettings }: Props) {
  const [dismissed, setDismissed] = useState(
    () => localStorage.getItem(DISMISS_KEY) === "1"
  );

  if (!visible || dismissed) return null;

  const dismiss = () => setDismissed(true);

  const dismissForever = () => {
    localStorage.setItem(DISMISS_KEY, "1");
    setDismissed(true);
  };

  return (
    <div className="notif-banner">
      <span className="notif-text" onClick={onSettings}>
        ⚠️ Add LLM API key in Settings for AI commit summaries
      </span>
      <span className="notif-actions">
        <button className="notif-hide" onClick={dismissForever}>don't show again</button>
        <button className="notif-close" onClick={dismiss}><X size={10} /></button>
      </span>
    </div>
  );
}
