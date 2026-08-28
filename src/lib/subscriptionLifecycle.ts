export type ExternalDisposer = () => void;

/**
 * Owns an asynchronously-created external subscription across React cleanup.
 *
 * Tauri's `listen` resolves after the native listener has been registered. In
 * React StrictMode an effect can be cleaned up before that promise resolves;
 * without this owner the late unlisten function is lost and the native
 * listener survives the remount.
 */
export function ownAsyncDisposer(registration: Promise<ExternalDisposer>): ExternalDisposer {
  let disposed = false;
  let unlisten: ExternalDisposer | null = null;

  void registration.then(
    (nextUnlisten) => {
      if (disposed) {
        nextUnlisten();
        return;
      }
      unlisten = nextUnlisten;
    },
    () => {
      // Registration failure leaves no external resource to clean up. The
      // command/event owner remains responsible for any user-facing error.
    },
  );

  return () => {
    if (disposed) return;
    disposed = true;
    unlisten?.();
    unlisten = null;
  };
}

export function disposeAll(disposers: readonly ExternalDisposer[]): void {
  for (const dispose of disposers) dispose();
}
