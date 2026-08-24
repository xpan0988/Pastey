export const MAX_TERMINAL_INPUT_FRAME_BYTES = 8 * 1024;
export const MAX_QUEUED_TERMINAL_INPUT_BYTES = 64 * 1024;

export const DEVELOPER_TERMINAL_OUTPUT_EVENT = "pastey://developer-terminal-output";

export interface DeveloperTerminalOutputEvent {
  roomId: string;
  terminalSessionId: string;
  sequence: number;
  dataBase64: string;
}

export class TerminalInputBackpressureError extends Error {
  constructor() {
    super("Developer terminal input queue is full. Wait for pending input to be delivered.");
    this.name = "TerminalInputBackpressureError";
  }
}

type TerminalInputSender = (frame: number[]) => Promise<unknown>;

export class OrderedTerminalInputWriter {
  private readonly queuedChunks: Uint8Array[] = [];
  private queuedHeadOffset = 0;
  private queuedByteCount = 0;
  private draining = false;
  private stopped = false;
  private idleWaiters: Array<() => void> = [];

  constructor(
    private readonly send: TerminalInputSender,
    private readonly onError: (error: unknown) => void,
    private readonly maxFrameBytes = MAX_TERMINAL_INPUT_FRAME_BYTES,
    private readonly maxQueuedBytes = MAX_QUEUED_TERMINAL_INPUT_BYTES,
  ) {}

  enqueue(bytes: readonly number[]): void {
    if (this.stopped || bytes.length === 0) return;
    if (bytes.length > this.maxQueuedBytes - this.queuedByteCount) {
      throw new TerminalInputBackpressureError();
    }
    this.queuedChunks.push(Uint8Array.from(bytes));
    this.queuedByteCount += bytes.length;
    void this.drain();
  }

  cancel(): void {
    this.stopped = true;
    this.clearQueuedInput();
    this.resolveIdleWaiters();
  }

  pendingBytes(): number {
    return this.queuedByteCount;
  }

  async whenIdle(): Promise<void> {
    if ((!this.draining && this.queuedByteCount === 0) || this.stopped) return;
    await new Promise<void>((resolve) => this.idleWaiters.push(resolve));
  }

  private async drain(): Promise<void> {
    if (this.draining || this.stopped) return;
    this.draining = true;
    try {
      while (!this.stopped && this.queuedByteCount > 0) {
        const frame = this.takeFrame();
        await this.send(frame);
      }
    } catch (error) {
      if (!this.stopped) {
        this.stopped = true;
        this.clearQueuedInput();
        this.onError(error);
      }
    } finally {
      this.draining = false;
      this.resolveIdleWaiters();
    }
  }

  private takeFrame(): number[] {
    const frameLength = Math.min(this.maxFrameBytes, this.queuedByteCount);
    const frame = new Uint8Array(frameLength);
    let written = 0;
    while (written < frameLength) {
      const chunk = this.queuedChunks[0];
      const available = chunk.length - this.queuedHeadOffset;
      const copied = Math.min(available, frameLength - written);
      frame.set(chunk.subarray(this.queuedHeadOffset, this.queuedHeadOffset + copied), written);
      written += copied;
      this.queuedHeadOffset += copied;
      this.queuedByteCount -= copied;
      if (this.queuedHeadOffset === chunk.length) {
        this.queuedChunks.shift();
        this.queuedHeadOffset = 0;
      }
    }
    return [...frame];
  }

  private clearQueuedInput(): void {
    this.queuedChunks.length = 0;
    this.queuedHeadOffset = 0;
    this.queuedByteCount = 0;
  }

  private resolveIdleWaiters(): void {
    if (this.draining || this.queuedByteCount > 0) return;
    const waiters = this.idleWaiters.splice(0);
    waiters.forEach((resolve) => resolve());
  }
}

export function terminalInputBytes(data: string): number[] {
  return [...new TextEncoder().encode(data)];
}

export function decodeTerminalOutputFrame(dataBase64: string): Uint8Array | null {
  try {
    const decoded = atob(dataBase64);
    if (decoded.length === 0 || decoded.length > MAX_TERMINAL_INPUT_FRAME_BYTES) return null;
    return Uint8Array.from(decoded, (character) => character.charCodeAt(0));
  } catch {
    return null;
  }
}
