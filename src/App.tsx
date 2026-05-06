import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { HomePage } from "./pages/HomePage";
import { RoomPage } from "./pages/RoomPage";
import { SettingsPage } from "./pages/SettingsPage";
import { burnRoom, getConfig, getRoom, leaveRoom, listRoomItems, listRooms } from "./lib/tauri";
import type { AppConfig, FileTransferProgressEvent, RoomInfo, RoomItem } from "./lib/types";

type View =
  | { screen: "home" }
  | { screen: "settings" }
  | { screen: "room"; roomId: string };

interface FocusPayload {
  target?: "home" | "settings";
}

function App() {
  const [view, setView] = useState<View>({ screen: "home" });
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [rooms, setRooms] = useState<RoomInfo[]>([]);
  const [currentRoom, setCurrentRoom] = useState<RoomInfo | null>(null);
  const [roomItems, setRoomItems] = useState<RoomItem[]>([]);
  const [transfers, setTransfers] = useState<Record<string, FileTransferProgressEvent>>({});
  const [focusToken, setFocusToken] = useState(0);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    async function load() {
      try {
        const [nextConfig, nextRooms] = await Promise.all([getConfig(), listRooms()]);
        setConfig(nextConfig);
        setRooms(nextRooms);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    }

    void load();
  }, []);

  useEffect(() => {
    let unlistenFocus: (() => void) | undefined;
    let unlistenTransfer: (() => void) | undefined;

    void listen<FocusPayload>("pastey://focus", (event) => {
      const target = event.payload.target ?? "home";
      setView(target === "settings" ? { screen: "settings" } : { screen: "home" });
      setFocusToken((value) => value + 1);
    }).then((fn) => {
      unlistenFocus = fn;
    });

    void listen<FileTransferProgressEvent>("pastey://transfer-progress", (event) => {
      setTransfers((current) => ({
        ...current,
        [event.payload.transfer_id]: event.payload
      }));
      if (event.payload.status === "completed") {
        void refreshCurrentRoom();
      }
    }).then((fn) => {
      unlistenTransfer = fn;
    });

    return () => {
      if (unlistenFocus) unlistenFocus();
      if (unlistenTransfer) unlistenTransfer();
    };
  }, [view]);

  async function refreshRooms(selectedRoomId?: string): Promise<RoomInfo | null> {
    const nextRooms = await listRooms();
    setRooms(nextRooms);

    if (selectedRoomId) {
      const match = nextRooms.find((room) => room.id === selectedRoomId) ?? null;
      setCurrentRoom(match);
      return match;
    }

    return null;
  }

  async function openRoom(room: RoomInfo) {
    setView({ screen: "room", roomId: room.id });
    try {
      const [nextRoom, nextItems] = await Promise.all([getRoom(room.id), listRoomItems(room.id)]);
      setCurrentRoom(nextRoom);
      setRoomItems(nextItems);
      await refreshRooms(room.id);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function refreshCurrentRoom() {
    if (view.screen !== "room") return;
    try {
      const [nextRoom, nextItems] = await Promise.all([getRoom(view.roomId), listRoomItems(view.roomId)]);
      setCurrentRoom(nextRoom);
      setRoomItems(nextItems);
      const visibleRoom = await refreshRooms(view.roomId);
      if (!visibleRoom) {
        setView({ screen: "home" });
        setRoomItems([]);
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (
        message === "room not found" ||
        message === "File is no longer available" ||
        message === "File is no longer available." ||
        message === "Peer left the room" ||
        message === "Peer left the room."
      ) {
        setView({ screen: "home" });
        setCurrentRoom(null);
        setRoomItems([]);
        return;
      }

      setError(message);
    }
  }

  async function handleBurnRoom(roomId: string) {
    await burnRoom(roomId);
    setView({ screen: "home" });
    setCurrentRoom(null);
    setRoomItems([]);
    setTransfers((current) => Object.fromEntries(Object.entries(current).filter(([, transfer]) => transfer.room_id !== roomId)));
    await refreshRooms();
  }

  async function handleLeaveRoom(roomId: string) {
    await leaveRoom(roomId);
    setView({ screen: "home" });
    setCurrentRoom(null);
    setRoomItems([]);
    setTransfers((current) => Object.fromEntries(Object.entries(current).filter(([, transfer]) => transfer.room_id !== roomId)));
    await refreshRooms();
  }

  if (!config) {
    return (
      <div className="app-shell center-panel">
        <div className="panel">{error ?? "Loading pastey..."}</div>
      </div>
    );
  }

  return (
    <div className="app-shell">
      {error ? <div className="error-box">{error}</div> : null}

      <main>
        {view.screen === "home" ? (
          <HomePage
            config={config}
            rooms={rooms}
            onOpenRoom={(room) => void openRoom(room)}
            onShowSettings={() => setView({ screen: "settings" })}
            shouldFocus={focusToken > 0}
          />
        ) : null}

        {view.screen === "room" && currentRoom ? (
          <RoomPage
            room={currentRoom}
            items={roomItems}
            transfers={Object.values(transfers).filter((transfer) => transfer.room_id === currentRoom.id)}
            onBack={() => {
              setView({ screen: "home" });
              void refreshRooms();
            }}
            onRefresh={refreshCurrentRoom}
            onBurn={handleBurnRoom}
            onLeave={handleLeaveRoom}
          />
        ) : null}

        {view.screen === "settings" ? (
          <div className="stack">
            <button className="text-button back-button settings-back" onClick={() => setView({ screen: "home" })}>
              Back
            </button>
            <SettingsPage config={config} onConfigChange={setConfig} />
          </div>
        ) : null}
      </main>
    </div>
  );
}

export default App;
