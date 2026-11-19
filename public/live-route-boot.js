(function () {
  if (!/^\/live\/[A-Za-z0-9._-]+/i.test(location.pathname)) return;
  const title = document.getElementById("flight-name");
  if (title) title.textContent = "Live session";
  const status = document.getElementById("status");
  if (status) {
    status.hidden = true;
    status.textContent = "";
  }
})();