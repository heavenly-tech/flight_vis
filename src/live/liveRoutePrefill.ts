import { parseLivePath } from "./runtime";

const live = parseLivePath(window.location.pathname);
if (live.sessionId) {
  const title = document.getElementById("flight-name");
  if (title) title.textContent = "Live session";
  const status = document.getElementById("status");
  if (status) {
    status.hidden = true;
    status.textContent = "";
  }
}