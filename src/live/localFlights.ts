import type { FlightTag, TrimRange } from "../track/types";

const DB_NAME = "flight_vis_local";
const STORE = "flights";
const DB_VERSION = 1;

export type LocalFlightMeta = {
  id: string;
  name: string;
  fileName: string;
  updatedAt: string;
  trimStartMs: number;
  trimEndMs: number;
  tags: FlightTag[];
  size: number;
};

export type LocalFlightRecord = LocalFlightMeta & {
  track: string;
};

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE)) {
        db.createObjectStore(STORE, { keyPath: "id" });
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error("IndexedDB open failed"));
  });
}

export async function listLocalFlights(): Promise<LocalFlightMeta[]> {
  const db = await openDb();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE, "readonly");
    const store = tx.objectStore(STORE);
    const req = store.getAll();
    req.onsuccess = () => {
      const rows = (req.result as LocalFlightRecord[]).map(({ track, tags, ...meta }) => ({
        ...meta,
        tags,
        size: track.length,
      }));
      rows.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
      resolve(rows);
    };
    req.onerror = () => reject(req.error ?? new Error("Could not list local flights"));
  });
}

export async function getLocalFlight(id: string): Promise<LocalFlightRecord | null> {
  const db = await openDb();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE, "readonly");
    const req = tx.objectStore(STORE).get(id);
    req.onsuccess = () => resolve((req.result as LocalFlightRecord) ?? null);
    req.onerror = () => reject(req.error ?? new Error("Could not read local flight"));
  });
}

export async function putLocalFlight(input: {
  name: string;
  fileName: string;
  track: string;
  trim: TrimRange;
  tags: FlightTag[];
  id?: string;
}): Promise<LocalFlightMeta> {
  const id = input.id ?? `local:${crypto.randomUUID()}`;
  const record: LocalFlightRecord = {
    id,
    name: input.name,
    fileName: input.fileName,
    track: input.track,
    trimStartMs: input.trim.startMs,
    trimEndMs: input.trim.endMs,
    tags: input.tags,
    updatedAt: new Date().toISOString(),
    size: input.track.length,
  };
  const db = await openDb();
  await new Promise<void>((resolve, reject) => {
    const tx = db.transaction(STORE, "readwrite");
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error ?? new Error("Could not save local flight"));
    tx.objectStore(STORE).put(record);
  });
  const { track: _t, ...meta } = record;
  return meta;
}

export async function deleteLocalFlight(id: string): Promise<void> {
  const db = await openDb();
  await new Promise<void>((resolve, reject) => {
    const tx = db.transaction(STORE, "readwrite");
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error ?? new Error("Could not delete local flight"));
    tx.objectStore(STORE).delete(id);
  });
}

export function isLocalSlug(slug: string): boolean {
  return slug.startsWith("local:");
}