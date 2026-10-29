import { resolveObjectURL } from "node:buffer";
import { Worker } from "node:worker_threads";

import { vi } from "vitest";

// Run the browser bundle's generated Blob workers in real threads during Node tests.
export async function withWebWorkers<T>(run: () => Promise<T>): Promise<T> {
  const threads = new Set<Worker>();
  class BlobWorker {
    onmessage?: (event: { data: unknown }) => void;
    private readonly thread: Promise<Worker>;

    constructor(url: string) {
      const blob = resolveObjectURL(url);
      if (!blob) throw new Error(`Missing worker Blob: ${url}`);
      this.thread = blob.text().then((source) => {
        const thread = new Worker(
          `const { parentPort } = require("node:worker_threads");
globalThis.self = globalThis;
globalThis.postMessage = (data, transfer) => parentPort.postMessage(data, transfer);
globalThis.addEventListener = (type, listener) => {
  if (type === "error") process.on("uncaughtException", error => listener({ error }));
};
parentPort.on("message", data => globalThis.onmessage({ data }));
${source}`,
          { eval: true },
        );
        threads.add(thread);
        thread.on("message", (data: unknown) => this.onmessage?.({ data }));
        return thread;
      });
    }

    async postMessage(data: unknown, transfer: ArrayBuffer[] = []): Promise<void> {
      (await this.thread).postMessage(data, transfer);
    }

    async terminate(): Promise<void> {
      await (await this.thread).terminate();
    }
  }

  vi.stubGlobal("Worker", BlobWorker);
  try {
    return await run();
  } finally {
    await Promise.all([...threads].map((thread) => thread.terminate()));
    vi.unstubAllGlobals();
  }
}
