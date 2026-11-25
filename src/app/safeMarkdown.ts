/**
 * Minimal DOM-only safe Markdown renderer for /app.html copy and release notes.
 *
 * Security invariants:
 * - Never uses innerHTML / outerHTML / insertAdjacentHTML.
 * - All raw text (including HTML tags, scripts, attributes) is emitted via createTextNode.
 * - Links are restricted to https:// URLs or safe relative paths (/, ./, ../, #).
 */

const MAX_INLINE_DEPTH = 4;
const UNORDERED_ITEM_RE = /^\s*[-*]\s+(.+)$/;
const ORDERED_ITEM_RE = /^\s*\d+\.\s+(.+)$/;
const CONTROL_OR_SPACE_RE = /[\u0000-\u001f\u007f\s]/;

export function isSafeHref(raw: string): string | null {
  const trimmed = raw.trim();
  if (!trimmed || CONTROL_OR_SPACE_RE.test(trimmed)) {
    return null;
  }
  if (trimmed.startsWith("/")) {
    // Reject protocol-relative (//evil.example) and backslash variants (/\evil.example).
    if (trimmed.startsWith("//") || trimmed.startsWith("/\\")) {
      return null;
    }
    return trimmed;
  }
  if (trimmed.startsWith("./") || trimmed.startsWith("../") || trimmed.startsWith("#")) {
    return trimmed;
  }
  if (/^https:\/\//i.test(trimmed)) {
    try {
      const parsed = new URL(trimmed);
      if (parsed.protocol === "https:" && parsed.hostname.length > 0) {
        return parsed.href;
      }
    } catch {
      return null;
    }
  }
  return null;
}

type InlineOpts = {
  allowLinks: boolean;
  depth: number;
};

function appendText(frag: DocumentFragment, doc: Document, text: string): void {
  if (text.length > 0) {
    frag.appendChild(doc.createTextNode(text));
  }
}

function parseInline(input: string, doc: Document, opts: InlineOpts): DocumentFragment {
  const frag = doc.createDocumentFragment();
  if (!input) return frag;
  if (opts.depth >= MAX_INLINE_DEPTH) {
    appendText(frag, doc, input);
    return frag;
  }

  let i = 0;
  let plainStart = 0;

  const flushPlain = (end: number): void => {
    if (end > plainStart) {
      appendText(frag, doc, input.slice(plainStart, end));
    }
  };

  while (i < input.length) {
    if (input.startsWith("**", i)) {
      const close = input.indexOf("**", i + 2);
      if (close > i + 2) {
        flushPlain(i);
        const strong = doc.createElement("strong");
        strong.appendChild(
          parseInline(input.slice(i + 2, close), doc, {
            allowLinks: opts.allowLinks,
            depth: opts.depth + 1,
          }),
        );
        frag.appendChild(strong);
        i = close + 2;
        plainStart = i;
        continue;
      }
      i += 2;
      continue;
    }

    if (input[i] === "`") {
      const close = input.indexOf("`", i + 1);
      if (close > i + 1) {
        flushPlain(i);
        const code = doc.createElement("code");
        code.textContent = input.slice(i + 1, close);
        frag.appendChild(code);
        i = close + 1;
        plainStart = i;
        continue;
      }
      i += 1;
      continue;
    }

    if (input[i] === "[") {
      const closeBracket = input.indexOf("]", i + 1);
      if (closeBracket > i + 1 && input[closeBracket + 1] === "(") {
        const closeParen = input.indexOf(")", closeBracket + 2);
        if (closeParen > closeBracket + 2) {
          const label = input.slice(i + 1, closeBracket);
          const rawUrl = input.slice(closeBracket + 2, closeParen);
          const safeUrl = opts.allowLinks ? isSafeHref(rawUrl) : null;
          flushPlain(i);
          if (safeUrl) {
            const a = doc.createElement("a");
            a.setAttribute("href", safeUrl);
            if (/^https:\/\//i.test(safeUrl)) {
              a.setAttribute("rel", "noopener noreferrer");
            }
            a.appendChild(
              parseInline(label, doc, {
                allowLinks: false,
                depth: opts.depth + 1,
              }),
            );
            frag.appendChild(a);
          } else {
            appendText(frag, doc, input.slice(i, closeParen + 1));
          }
          i = closeParen + 1;
          plainStart = i;
          continue;
        }
      }
      i += 1;
      continue;
    }

    i += 1;
  }

  flushPlain(input.length);
  return frag;
}

export function renderSafeInlineMarkdown(
  input: string,
  doc: Document = document,
): DocumentFragment {
  return parseInline(input, doc, { allowLinks: true, depth: 0 });
}

export function renderSafeMarkdown(
  input: string,
  doc: Document = document,
): DocumentFragment {
  const frag = doc.createDocumentFragment();
  const normalized = input.replace(/\r\n?/g, "\n");
  const lines = normalized.split("\n");

  let paragraphLines: string[] = [];
  let activeList: HTMLUListElement | HTMLOListElement | null = null;
  let activeListTag: "ul" | "ol" | null = null;

  const flushParagraph = (): void => {
    if (paragraphLines.length === 0) return;
    const p = doc.createElement("p");
    p.appendChild(renderSafeInlineMarkdown(paragraphLines.join(" "), doc));
    frag.appendChild(p);
    paragraphLines = [];
  };

  const flushList = (): void => {
    if (!activeList) return;
    frag.appendChild(activeList);
    activeList = null;
    activeListTag = null;
  };

  for (const rawLine of lines) {
    const trimmed = rawLine.trim();
    if (!trimmed) {
      flushParagraph();
      flushList();
      continue;
    }

    const ulMatch = rawLine.match(UNORDERED_ITEM_RE);
    if (ulMatch) {
      flushParagraph();
      if (activeListTag !== "ul") {
        flushList();
        activeList = doc.createElement("ul");
        activeListTag = "ul";
      }
      const li = doc.createElement("li");
      li.appendChild(renderSafeInlineMarkdown(ulMatch[1].trim(), doc));
      activeList!.appendChild(li);
      continue;
    }

    const olMatch = rawLine.match(ORDERED_ITEM_RE);
    if (olMatch) {
      flushParagraph();
      if (activeListTag !== "ol") {
        flushList();
        activeList = doc.createElement("ol");
        activeListTag = "ol";
      }
      const li = doc.createElement("li");
      li.appendChild(renderSafeInlineMarkdown(olMatch[1].trim(), doc));
      activeList!.appendChild(li);
      continue;
    }

    flushList();
    paragraphLines.push(trimmed);
  }

  flushParagraph();
  flushList();
  return frag;
}

export function renderMarkdownInto(target: HTMLElement, text: string): void {
  const doc = target.ownerDocument;
  const trimmed = text.trim();
  if (target.id) {
    const existingSibling = target.parentElement?.querySelector<HTMLDivElement>(
      `[data-md-for="${target.id}"]`,
    );
    existingSibling?.remove();
  }

  if (target.tagName === "P") {
    const hasBlocks =
      /\n\s*\n/.test(trimmed) ||
      UNORDERED_ITEM_RE.test(trimmed) ||
      ORDERED_ITEM_RE.test(trimmed) ||
      trimmed.split(/\r\n?|\n/).some((line) => UNORDERED_ITEM_RE.test(line) || ORDERED_ITEM_RE.test(line));
    if (!hasBlocks) {
      target.hidden = false;
      target.replaceChildren(renderSafeInlineMarkdown(trimmed, doc));
      return;
    }
    const box = doc.createElement("div");
    box.className = "release-markdown";
    if (target.id) {
      box.dataset.mdFor = target.id;
    }
    box.appendChild(renderSafeMarkdown(trimmed, doc));
    target.replaceChildren();
    target.hidden = true;
    target.insertAdjacentElement("afterend", box);
    return;
  }

  target.hidden = false;
  target.replaceChildren(renderSafeMarkdown(trimmed, doc));
}
