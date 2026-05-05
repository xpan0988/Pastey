import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";

interface DropZoneProps {
  onPick: (path: string) => Promise<void>;
}

export function DropZone({ onPick }: DropZoneProps) {
  const [isHovering, setIsHovering] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    void getCurrentWebview()
      .onDragDropEvent(async (event) => {
        if (event.payload.type === "over") {
          setIsHovering(true);
          return;
        }

        if (event.payload.type === "drop") {
          setIsHovering(false);
          const [firstPath] = event.payload.paths;
          if (!firstPath) return;
          setBusy(true);
          try {
            await onPick(firstPath);
          } finally {
            setBusy(false);
          }
          return;
        }

        setIsHovering(false);
      })
      .then((fn) => {
        unlisten = fn;
      });

    return () => {
      if (unlisten) {
        unlisten();
      }
    };
  }, [onPick]);

  async function handleBrowse() {
    const selected = await open({
      multiple: false,
      directory: false
    });

    if (typeof selected === "string") {
      setBusy(true);
      try {
        await onPick(selected);
      } finally {
        setBusy(false);
      }
    }
  }

  return (
    <div className={`drop-zone ${isHovering ? "hover" : ""}`}>
      <div className="subtle-stack">
        <strong>{busy ? "Encrypting..." : "Drop a file or image here"}</strong>
        <p className="muted">Files are encrypted locally before they are sent into the room.</p>
      </div>
      <button className="ghost-button" onClick={handleBrowse} disabled={busy}>
        Choose file
      </button>
    </div>
  );
}
