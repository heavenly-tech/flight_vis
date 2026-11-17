import { describe, expect, it } from "vitest";
import { createIngestQueue } from "./ingestQueue";

describe("createIngestQueue", () => {
  it("runs ingest batches serially", async () => {
    const order: number[] = [];
    const queue = createIngestQueue(async (n: number) => {
      order.push(n);
      await new Promise((r) => setTimeout(r, n === 1 ? 30 : 5));
      order.push(n + 10);
    });
    void queue.enqueue(1);
    void queue.enqueue(2);
    await queue.idle();
    expect(order).toEqual([1, 11, 2, 12]);
  });

  it("continues after a worker throws", async () => {
    const order: string[] = [];
    const queue = createIngestQueue(async (label: string) => {
      order.push(label);
      if (label === "bad") throw new Error("boom");
    });
    void queue.enqueue("ok");
    void queue.enqueue("bad");
    void queue.enqueue("ok2");
    await queue.idle();
    expect(order).toEqual(["ok", "bad", "ok2"]);
  });
});