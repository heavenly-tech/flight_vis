import { detectLang, msg, type LiveLang } from "./messages";
import { connectLiveStream, type LiveStreamErrorState, type LiveStreamHandle } from "./sseClient";

export type LivePageOptions = {
  sessionId: string;
  statusEl: HTMLElement;
  countEl: HTMLElement;
  locale?: LiveLang;
  eventSourceFactory?: (url: string) => EventSource;
};

export class LivePage {
  private readonly opts: LivePageOptions;
  private handle: LiveStreamHandle | null = null;
  private fixCount = 0;

  constructor(opts: LivePageOptions) {
    this.opts = opts;
  }

  connect(): void {
    const locale = this.opts.locale ?? detectLang();
    this.setStatus(msg("reconnecting", locale));
    this.handle?.close();
    this.handle = connectLiveStream(
      this.opts.sessionId,
      {
        onFix: () => {
          this.fixCount += 1;
          this.opts.countEl.textContent = String(this.fixCount);
          this.setStatus(`fixes:${this.fixCount}`);
        },
        onEnd: () => {
          this.setStatus(msg("sessionEnded", locale));
        },
        onError: (state: LiveStreamErrorState) => {
          this.setStatus(this.errorText(locale, state));
        },
      },
      this.opts.eventSourceFactory,
    );
  }

  disconnect(): void {
    this.handle?.close();
    this.handle = null;
  }

  private setStatus(text: string): void {
    this.opts.statusEl.textContent = text;
  }

  private errorText(locale: LiveLang, state: LiveStreamErrorState): string {
    if (state === "auth") return msg("loginRequired", locale);
    if (state === "notfound") return msg("notFound", locale);
    return msg("reconnecting", locale);
  }
}