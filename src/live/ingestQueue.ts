export function createIngestQueue<T>(worker: (batch: T) => Promise<void>): {
  enqueue: (batch: T) => Promise<void>;
  idle: () => Promise<void>;
} {
  let tail: Promise<void> = Promise.resolve();
  return {
    enqueue(batch: T) {
      tail = tail.then(() => worker(batch));
      return tail;
    },
    idle() {
      return tail;
    },
  };
}