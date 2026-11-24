/**
 * Scans raw JSON text for duplicate object keys at any nesting level.
 * Returns dotted paths of all duplicate keys found.
 * Does NOT use JSON.parse - works on raw text to catch duplicates that
 * would be lost during parsing.
 */
export function findDuplicateKeys(text: string): string[] {
  const seen: Map<string, number> = new Map();
  const duplicates: Set<string> = new Set();
  
  // Stack of object contexts. Each entry is the path prefix for that object.
  const contextStack: string[] = [];
  let expectingKey = false;
  let pendingKey: string | null = null;
  
  let i = 0;
  while (i < text.length) {
    const c = text[i];

    if (c === '"') {
      // Start of a string
      const start = i;
      i++;
      let inEscape = false;
      let strEnd = -1;

      while (i < text.length) {
        if (inEscape) {
          inEscape = false;
          i++;
          continue;
        }
        if (text[i] === '\\') {
          inEscape = true;
          i++;
          continue;
        }
        if (text[i] === '"') {
          strEnd = i;
          i++;
          break;
        }
        i++;
      }

      if (strEnd === -1) break; // Unclosed string

      const str = text.slice(start + 1, strEnd);

      // Check if this string is a key (we're expecting a key and it's followed by :)
      if (expectingKey) {
        // Skip whitespace after the closing quote
        let j = i;
        while (j < text.length && (text[j] === ' ' || text[j] === '\t' || text[j] === '\n' || text[j] === '\r')) {
          j++;
        }

        if (j < text.length && text[j] === ':') {
          // This is a key
          const fullPath = contextStack.length > 0 
            ? `${contextStack.join('.')}.${str}`
            : str;
          
          const count = seen.get(fullPath) ?? 0;
          if (count === 1) {
            duplicates.add(fullPath);
          }
          seen.set(fullPath, count + 1);
          
          // Store this as the pending key for when we enter a nested object
          pendingKey = str;
          expectingKey = false;
          i = j + 1; // Skip past the :
          continue;
        }
      }
      // If we're not expecting a key, this is a string value - continue
    } else if (c === '{') {
      // Start of object
      if (pendingKey !== null) {
        // We're entering a nested object, push the pending key to the context
        contextStack.push(pendingKey);
        pendingKey = null;
      }
      expectingKey = true;
      i++;
    } else if (c === '}') {
      // End of object
      contextStack.pop();
      pendingKey = null;
      expectingKey = false;
      i++;
    } else if (c === '[') {
      // Start of array - push a marker
      contextStack.push('[]');
      expectingKey = false;
      i++;
    } else if (c === ']') {
      // End of array
      contextStack.pop();
      i++;
    } else if (c === ':') {
      // After a key comes a value
      expectingKey = false;
      i++;
      continue;
    } else if (c === ',') {
      // After a value, we might see another key
      i++;
      // Skip whitespace
      while (i < text.length && (text[i] === ' ' || text[i] === '\t' || text[i] === '\n' || text[i] === '\r')) {
        i++;
      }
      
      // Check if we're in an object (not an array)
      if (contextStack.length === 0 || contextStack[contextStack.length - 1] !== '[]') {
        expectingKey = true;
        pendingKey = null;
      }
      continue;
    } else {
      i++;
    }
  }

  return Array.from(duplicates).sort();
}
