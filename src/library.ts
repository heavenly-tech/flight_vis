import {
  deleteFlight,
  listFlights,
  loginUrl,
  renameFlight,
  SlugConflictError,
  uploadFlights,
  type AuthStatus,
  type FlightListItem,
} from "./api";
import { isValidSlug, nameFromFileName, normalizeSlug } from "./track/slug";

export function isTrackFile(file: File): boolean {
  return /\.(igc|gpx)$/i.test(file.name);
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatWhen(iso: string | null): string {
  if (!iso) return "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

export type LibraryHandle = {
  refresh: () => Promise<void>;
  setOpen: (open: boolean) => void;
  isOpen: () => boolean;
  setCurrentSlug: (slug: string | null) => void;
  setAuth: (next: AuthStatus) => void;
  saveFiles: (files: File[]) => Promise<FlightListItem[]>;
};

export function mountLibrary(opts: {
  getAuth: () => AuthStatus;
  currentSlug: () => string | null;
  onOpenFlight: (slug: string) => Promise<void>;
  onRenamed: (from: string, to: FlightListItem) => void;
  onDeleted: (slug: string) => void;
  setStatus: (text: string | null) => void;
}): LibraryHandle {
  const panel = document.querySelector<HTMLElement>("#library-panel")!;
  const button = document.querySelector<HTMLButtonElement>("#btn-library")!;
  const closeButton = document.querySelector<HTMLButtonElement>("#btn-library-close")!;
  const hint = document.querySelector<HTMLParagraphElement>("#library-hint")!;
  const tools = document.querySelector<HTMLElement>("#library-tools")!;
  const loginLink = document.querySelector<HTMLAnchorElement>("#library-login")!;
  const listEl = document.querySelector<HTMLUListElement>("#library-list")!;
  const input = document.querySelector<HTMLInputElement>("#library-input")!;
  const renameDialog = document.querySelector<HTMLDialogElement>("#rename-dialog")!;
  const renameForm = document.querySelector<HTMLFormElement>("#rename-form")!;
  const renameName = document.querySelector<HTMLInputElement>("#rename-name")!;
  const renameSlug = document.querySelector<HTMLInputElement>("#rename-slug")!;
  const renameWarn = document.querySelector<HTMLParagraphElement>("#rename-warn")!;
  const renameOverwrite = document.querySelector<HTMLButtonElement>("#rename-overwrite")!;
  const renameCancel = document.querySelector<HTMLButtonElement>("#rename-cancel")!;
  const deleteDialog = document.querySelector<HTMLDialogElement>("#delete-dialog")!;
  const deleteForm = document.querySelector<HTMLFormElement>("#delete-form")!;
  const deleteLabel = document.querySelector<HTMLElement>("#delete-label")!;

  let items: FlightListItem[] = [];
  let currentSlug: string | null = opts.currentSlug();
  let renaming: FlightListItem | null = null;
  let deleting: FlightListItem | null = null;

  function setOpen(open: boolean): void {
    panel.hidden = !open;
    button.setAttribute("aria-expanded", String(open));
    document.body.classList.toggle("library-open", open);
    window.dispatchEvent(new Event("resize"));
    if (open) void refresh();
  }

  function flash(text: string): void {
    opts.setStatus(text);
    window.setTimeout(() => opts.setStatus(null), 2800);
  }

  function render(): void {
    const auth = opts.getAuth();
    loginLink.href = loginUrl();
    loginLink.hidden = auth.authenticated;
    tools.hidden = !auth.authenticated;
    if (!auth.configured) {
      hint.textContent = "Upload is locked until FLIGHT_VIS_PASSWORD is set.";
    } else if (!auth.authenticated) {
      hint.textContent = "Log in to list, upload, rename, or delete saved tracks.";
    } else {
      hint.textContent = items.length ? `${items.length} saved track${items.length === 1 ? "" : "s"}` : "No saved tracks yet.";
    }

    listEl.innerHTML = "";
    if (!auth.authenticated) return;

    for (const item of items) {
      const row = document.createElement("li");
      row.className = "library-item";
      if (item.slug === currentSlug) row.classList.add("active");

      const open = document.createElement("button");
      open.type = "button";
      open.className = "library-open";
      const title = document.createElement("strong");
      title.textContent = item.name || item.slug;
      const meta = document.createElement("span");
      meta.textContent = [item.slug, formatSize(item.size), formatWhen(item.updatedAt)].filter(Boolean).join(" · ");
      open.append(title, meta);
      open.addEventListener("click", () => {
        void opts.onOpenFlight(item.slug);
      });

      const actions = document.createElement("div");
      actions.className = "library-item-actions";
      const renameBtn = document.createElement("button");
      renameBtn.type = "button";
      renameBtn.className = "chip";
      renameBtn.textContent = "Rename";
      renameBtn.addEventListener("click", () => openRename(item));
      const deleteBtn = document.createElement("button");
      deleteBtn.type = "button";
      deleteBtn.className = "chip danger";
      deleteBtn.textContent = "Delete";
      deleteBtn.addEventListener("click", () => openDelete(item));
      actions.append(renameBtn, deleteBtn);

      row.append(open, actions);
      listEl.append(row);
    }
  }

  async function refresh(): Promise<void> {
    const auth = opts.getAuth();
    if (!auth.authenticated) {
      items = [];
      render();
      return;
    }
    try {
      items = await listFlights();
    } catch (error) {
      items = [];
      flash(error instanceof Error ? error.message : "Could not list flights");
    }
    render();
  }

  function openRename(item: FlightListItem): void {
    renaming = item;
    renameName.value = item.name;
    renameSlug.value = item.slug;
    renameWarn.hidden = true;
    renameWarn.textContent = "";
    renameOverwrite.hidden = true;
    renameDialog.showModal();
    renameName.focus();
    renameName.select();
  }

  function openDelete(item: FlightListItem): void {
    deleting = item;
    deleteLabel.textContent = `${item.name} (${item.slug})`;
    deleteDialog.showModal();
  }

  async function applyRename(overwrite: boolean): Promise<void> {
    if (!renaming) return;
    const name = renameName.value.trim();
    const slug = normalizeSlug(renameSlug.value);
    if (!name) {
      renameWarn.hidden = false;
      renameWarn.textContent = "Name is required.";
      return;
    }
    if (!isValidSlug(slug)) {
      renameWarn.hidden = false;
      renameWarn.textContent = "Slug must be lowercase letters, numbers, and dashes.";
      return;
    }
    try {
      const from = renaming.slug;
      const saved = await renameFlight(from, { name, slug, overwrite });
      renameDialog.close();
      renaming = null;
      opts.onRenamed(from, saved);
      await refresh();
    } catch (error) {
      if (error instanceof SlugConflictError) {
        renameWarn.hidden = false;
        renameWarn.textContent = `/${error.slug} already exists. Overwrite that flight?`;
        renameOverwrite.hidden = false;
        return;
      }
      renameWarn.hidden = false;
      renameWarn.textContent = error instanceof Error ? error.message : "Could not rename";
    }
  }

  async function saveFiles(files: File[]): Promise<FlightListItem[]> {
    const auth = opts.getAuth();
    if (!auth.authenticated) {
      window.location.href = loginUrl();
      return [];
    }
    const tracks = [];
    const skipped: string[] = [];
    for (const file of files) {
      if (!isTrackFile(file)) {
        skipped.push(file.name);
        continue;
      }
      tracks.push({
        fileName: file.name,
        track: await file.text(),
        name: nameFromFileName(file.name),
      });
    }
    if (!tracks.length) {
      flash(skipped.length ? "Drop .igc or .gpx tracks." : "No files to upload.");
      return [];
    }
    try {
      const result = await uploadFlights(tracks);
      await refresh();
      const last = result.items.at(-1);
      if (last) await opts.onOpenFlight(last.slug);
      const extra = skipped.length ? ` Skipped ${skipped.length} non-track file${skipped.length === 1 ? "" : "s"}.` : "";
      flash(`Saved ${result.items.length} track${result.items.length === 1 ? "" : "s"}.${extra}`);
      return result.items;
    } catch (error) {
      flash(error instanceof Error ? error.message : "Could not upload");
      return [];
    }
  }

  button.addEventListener("click", () => {
    setOpen(panel.hidden);
  });
  closeButton.addEventListener("click", () => setOpen(false));

  input.addEventListener("change", async () => {
    const files = [...(input.files ?? [])];
    input.value = "";
    if (files.length) await saveFiles(files);
  });

  panel.addEventListener("dragover", (event) => {
    event.preventDefault();
    event.stopPropagation();
    panel.classList.add("drop-hover");
  });
  panel.addEventListener("dragleave", () => panel.classList.remove("drop-hover"));
  panel.addEventListener("drop", (event) => {
    event.preventDefault();
    event.stopPropagation();
    panel.classList.remove("drop-hover");
    const files = [...(event.dataTransfer?.files ?? [])];
    if (files.length) void saveFiles(files);
  });

  renameForm.addEventListener("submit", (event) => {
    event.preventDefault();
    void applyRename(false);
  });
  renameOverwrite.addEventListener("click", () => {
    void applyRename(true);
  });
  renameCancel.addEventListener("click", () => {
    renameDialog.close();
    renaming = null;
  });

  deleteForm.addEventListener("submit", (event) => {
    const submitter = (event as SubmitEvent).submitter as HTMLButtonElement | null;
    if (submitter?.value !== "ok" || !deleting) {
      deleting = null;
      return;
    }
    const target = deleting;
    deleting = null;
    void (async () => {
      try {
        await deleteFlight(target.slug);
        opts.onDeleted(target.slug);
        await refresh();
        flash(`Deleted ${target.slug}`);
      } catch (error) {
        flash(error instanceof Error ? error.message : "Could not delete");
      }
    })();
  });

  window.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    if (renameDialog.open || deleteDialog.open) return;
    if (!panel.hidden) setOpen(false);
  });

  render();

  return {
    refresh,
    setOpen,
    isOpen: () => !panel.hidden,
    setCurrentSlug: (slug) => {
      currentSlug = slug;
      render();
    },
    setAuth: () => {
      render();
      if (opts.getAuth().authenticated && !panel.hidden) void refresh();
    },
    saveFiles,
  };
}
