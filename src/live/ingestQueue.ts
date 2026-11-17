export function createIngestQueue<T>(worker: (batch: T) => Promise<void>): {
  enqueue: (batch: T) => Promise<void>;
  idle: () => Promise<void>;
} {
  let tail: Promise<void> = Promise.resolve();
  return {
    enqueue(batch: T) {
      tail = tail
        .catch(() => undefined)
        .then(() => worker(batch));
      return tail;
    },
    idle() {
      return tail;
    },
  };
}