import { flightToGpx } from "../live/flightToGpx";
import type { Flight } from "../track/types";

const DB_NAME = "flight_vis";
const STORE = "flights";
const DB_VERSION = 1;

export type LocalFlightRecord = {
  slug: string;
  name: string;
  fileName: string;
  gpx: string;
  savedAt: number;
  pointCount: number;
};

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onerror = () => reject(req.error ?? new Error("Could not open IndexedDB"));
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE)) {
        db.createObjectStore(STORE, { keyPath: "slug" });
      }
    };
    req.onsuccess = () => resolve(req.result);
  });
}

function request<T>(req: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error("IndexedDB request failed"));
  });
}

export function liveSlugFromTime(ms: number = Date.now()): string {
  return `live-${ms}`;
}

export async function saveLocalFlight(flight: Flight, opts?: { slug?: string }): Promise<LocalFlightRecord> {
  const slug = opts?.slug ?? liveSlugFromTime();
  const gpx = flightToGpx(flight);
  const record: LocalFlightRecord = {
    slug,
    name: flight.name,
    fileName: flight.fileName,
    gpx,
    savedAt: Date.now(),
    pointCount: flight.points.length,
  };
  const db = await openDb();
  const tx = db.transaction(STORE, "readwrite");
  await request(tx.objectStore(STORE).put(record));
  return record;
}

export async function loadLocalFlight(slug: string): Promise<LocalFlightRecord | null> {
  const db = await openDb();
  const tx = db.transaction(STORE, "readonly");
  const row = await request(tx.objectStore(STORE).get(slug));
  return (row as LocalFlightRecord | undefined) ?? null;
}

export async function listLocalFlights(): Promise<LocalFlightRecord[]> {
  const db = await openDb();
  const tx = db.transaction(STORE, "readonly");
  const rows = (await request(tx.objectStore(STORE).getAll())) as LocalFlightRecord[];
  return rows.sort((a, b) => b.savedAt - a.savedAt);
}

export async function deleteLocalFlight(slug: string): Promise<void> {
  const db = await openDb();
  const tx = db.transaction(STORE, "readwrite");
  await request(tx.objectStore(STORE).delete(slug));
}