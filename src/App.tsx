import { listen } from "@tauri-apps/api/event";
import { useEffect, useMemo, useState } from "react";
import { HomePage } from "./pages/HomePage";
import { RoomPage } from "./pages/RoomPage";
import { SettingsPage } from "./pages/SettingsPage";
import { TrayStatus } from "./components/TrayStatus";
import { burnRoom, getConfig, getRoom, leaveRoom, listRoomItems, listRooms } from "./lib/tauri";
import type { AppConfig, RoomInfo, RoomItem } from "./lib/types";

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

    void listen<FocusPayload>("pastey://focus", (event) => {
      const target = event.payload.target ?? "home";
      setView(target === "settings" ? { screen: "settings" } : { screen: "home" });
      setFocusToken((value) => value + 1);
    }).then((fn) => {
      unlistenFocus = fn;
    });

    return () => {
      if (unlistenFocus) unlistenFocus();
    };
  }, []);

  const activeCount = useMemo(
    () => rooms.length,
    [rooms]
  );

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
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function handleBurnRoom(roomId: string) {
    await burnRoom(roomId);
    setView({ screen: "home" });
    setCurrentRoom(null);
    setRoomItems([]);
    await refreshRooms();
  }

  async function handleLeaveRoom(roomId: string) {
    await leaveRoom(roomId);
    setView({ screen: "home" });
    setCurrentRoom(null);
    setRoomItems([]);
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
      <header className="app-header">
        <div>
          <div className="brand">pastey</div>
          <p className="tagline">Encrypted room-based transfer for nearby devices you control.</p>
        </div>
        <TrayStatus activeCount={activeCount} />
      </header>

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
