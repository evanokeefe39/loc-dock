import { getCurrentWindow } from "@tauri-apps/api/window";

const DIRECTIONS = ["North", "South", "East", "West", "NorthEast", "NorthWest", "SouthEast", "SouthWest"] as const;
const CSS_MAP: Record<string, string> = {
  North: "resize-n",
  South: "resize-s",
  East: "resize-e",
  West: "resize-w",
  NorthEast: "resize-ne",
  NorthWest: "resize-nw",
  SouthEast: "resize-se",
  SouthWest: "resize-sw",
};

export function ResizeBorders() {
  const startResize = (direction: string) => {
    getCurrentWindow().startResizeDragging(direction as any);
  };

  return (
    <>
      {DIRECTIONS.map((dir) => (
        <div
          key={dir}
          className={`resize-border ${CSS_MAP[dir]}`}
          onMouseDown={() => startResize(dir)}
        />
      ))}
    </>
  );
}
