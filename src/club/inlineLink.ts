/**
 * Renders a localized string containing a `{link}` placeholder into `target`
 * using DOM APIs only (never `innerHTML`).
 */
export function renderTextWithLink(
  target: HTMLElement,
  template: string,
  href: string,
  linkLabel: string,
): void {
  const token = "{link}";
  const idx = template.indexOf(token);
  const link = document.createElement("a");
  link.href = href;
  link.textContent = linkLabel;
  if (idx === -1) {
    target.replaceChildren(document.createTextNode(`${template} `), link);
    return;
  }
  const before = template.slice(0, idx);
  const after = template.slice(idx + token.length);
  const nodes: Node[] = [];
  if (before) nodes.push(document.createTextNode(before));
  nodes.push(link);
  if (after) nodes.push(document.createTextNode(after));
  target.replaceChildren(...nodes);
}
