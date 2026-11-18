function safeNextPath(raw: string | null): string {
  const dest = raw || "/";
  if (!dest.startsWith("/") || dest.startsWith("//") || dest.includes("://")) {
    return "/";
  }
  return dest;
}

const q = new URLSearchParams(location.search);
const err = document.querySelector<HTMLParagraphElement>("#err")!;
const next = document.querySelector<HTMLInputElement>("#next")!;
const dest = safeNextPath(q.get("next"));
next.value = dest;

if (q.get("e") === "1") {
  err.textContent = "Wrong password.";
}

function showLocked(): void {
  const body = document.querySelector<HTMLDivElement>("#body");
  if (!body) return;
  body.innerHTML =
    '<p class="lock">Upload is locked. Administrator must set <code>FLIGHT_VIS_PASSWORD</code>.</p>';
}

if (q.get("locked") === "1") {
  showLocked();
}

document.getElementById("login-form")?.addEventListener("submit", (event) => {
  const form = event.currentTarget as HTMLFormElement;
  const button = form.querySelector("button");
  if (button) button.disabled = true;
});

void fetch("/api/auth/me", { credentials: "same-origin" })
  .then((res) => (res.ok ? res.json() : { authenticated: false, configured: true }))
  .then((me: { authenticated?: boolean; configured?: boolean }) => {
    if (me.authenticated) {
      location.replace(next.value || "/");
    } else if (me.configured === false) {
      showLocked();
    }
  })
  .catch(() => {});