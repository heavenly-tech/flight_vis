import { api, mountAdminNav, requireAdmin } from "./adminCommon";

const list = document.querySelector<HTMLUListElement>("#list")!;
const code = document.querySelector<HTMLInputElement>("#code")!;
const form = document.querySelector<HTMLFormElement>("#form")!;

async function refresh(): Promise<void> {
  const data = await api<{ types: { id: string; code: string }[] }>("/api/admin/aircraft-types");
  list.replaceChildren(
    ...data.types.map((t) => {
      const li = document.createElement("li");
      li.textContent = t.code;
      return li;
    }),
  );
}

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  await api("/api/admin/aircraft-types", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code: code.value.trim() }),
  });
  code.value = "";
  await refresh();
});

void (async () => {
  mountAdminNav();
  try {
    const who = document.querySelector<HTMLParagraphElement>("#who")!;
    const admin = await requireAdmin();
    who.textContent = `Admin: ${admin.name}`;
    await refresh();
  } catch {
    list.innerHTML = "<li>Sign in as admin at club login.</li>";
  }
})();