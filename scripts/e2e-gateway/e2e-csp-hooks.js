window.__cspViolations = [];
document.addEventListener("securitypolicyviolation", (ev) => {
  const where = ev.sourceFile ? ` @ ${ev.sourceFile}:${ev.lineNumber}` : "";
  window.__cspViolations.push(`${ev.violatedDirective}: ${ev.blockedURI || "inline"}${where}`);
});