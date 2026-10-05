/** Small numeric expressions for Inspector inputs; works under CSP without evaluating JS. */
export function evaluateNumber(source: string): number | null {
  if (!source.trim() || source.length > 256) return null;
  let position = 0;
  const skipSpace = () => { while (/\s/.test(source[position] ?? "") && position < source.length) position++; };
  const take = (token: string) => {
    skipSpace();
    if (!source.startsWith(token, position)) return false;
    position += token.length;
    return true;
  };
  const primary = (): number => {
    if (take("(")) {
      const value = sum();
      if (!take(")")) throw new SyntaxError("Missing closing parenthesis");
      return value;
    }
    skipSpace();
    const literal = /^(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?/.exec(source.slice(position));
    if (!literal) throw new SyntaxError("Expected a number");
    position += literal[0].length;
    return Number(literal[0]);
  };
  const power = (): number => {
    const value = primary();
    return take("**") ? value ** unary() : value;
  };
  const unary = (): number => take("+") ? unary() : take("-") ? -unary() : power();
  const product = (): number => {
    let value = unary();
    while (true) {
      if (take("*")) value *= unary();
      else if (take("/")) value /= unary();
      else return value;
    }
  };
  const sum = (): number => {
    let value = product();
    while (true) {
      if (take("+")) value += product();
      else if (take("-")) value -= product();
      else return value;
    }
  };
  try {
    const value = sum();
    skipSpace();
    return position === source.length && Number.isFinite(value) ? value : null;
  } catch {
    return null;
  }
}
