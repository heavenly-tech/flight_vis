export type LiveLang = "en" | "es";

export function detectLang(): LiveLang {
  const stored = localStorage.getItem("fv_live_lang");
  if (stored === "en" || stored === "es") return stored;
  const nav = navigator.language.toLowerCase();
  return nav.startsWith("es") ? "es" : "en";
}

export function setLiveLang(lang: LiveLang): void {
  localStorage.setItem("fv_live_lang", lang);
}

const COPY = {
  en: {
    connecting: "Connecting to live stream…",
    reconnecting: "Reconnecting…",
    sessionEnded: "Live session ended",
    notFound: "Live session not found",
    loginRequired: "Log in to watch this live session",
    followLive: "Live",
    recordOn: "Recording",
    recordOff: "Record",
    liveSessions: "Live",
    saveRecording: "Save recording",
    trimTitle: "Trim tail before save",
    cancel: "Cancel",
    save: "Save",
    recorded: "Saved to device library",
    noLiveSessions: "No active live sessions",
    pickSession: "Choose a live session",
    accuracyLabel: "GPS accuracy",
  },
  es: {
    connecting: "Conectando a la transmisión en vivo…",
    reconnecting: "Reconectando…",
    sessionEnded: "Sesión en vivo finalizada",
    notFound: "Sesión en vivo no encontrada",
    loginRequired: "Inicia sesión para ver esta transmisión",
    followLive: "En vivo",
    recordOn: "Grabando",
    recordOff: "Grabar",
    liveSessions: "En vivo",
    saveRecording: "Guardar grabación",
    trimTitle: "Recortar cola antes de guardar",
    cancel: "Cancelar",
    save: "Guardar",
    recorded: "Guardado en la biblioteca del dispositivo",
    noLiveSessions: "No hay sesiones en vivo",
    pickSession: "Elige una sesión en vivo",
    accuracyLabel: "Precisión GPS",
  },
} as const;

export function msg(key: keyof (typeof COPY)["en"], lang = detectLang()): string {
  return COPY[lang][key];
}