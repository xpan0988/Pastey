import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";

interface DropZoneProps {
  onPick: (path: string) => Promise<void>;
  disabled?: boolean;
}

export function DropZone({ onPick, disabled = false }: DropZoneProps) {
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
          if (disabled || busy) return;
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
  }, [busy, disabled, onPick]);

  async function handleBrowse() {
    if (disabled || busy) return;
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
        <strong>{busy ? "Transferring..." : "Drop a file or image here"}</strong>
        <p className="muted">Files stream in encrypted chunks over the local room. Max file size: 10GB.</p>
      </div>
      <button className="ghost-button" onClick={handleBrowse} disabled={busy || disabled}>
        Choose file
      </button>
    </div>
  );
}
