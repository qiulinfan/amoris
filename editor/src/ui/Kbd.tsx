import { keyCaps } from "../commands/keys";

export function Kbd({ keys }: { keys: string }) {
  return (
    <span className="kbd-group">
      {keyCaps(keys).map((k, i) => (
        <kbd key={i} className="kbd">
          {k}
        </kbd>
      ))}
    </span>
  );
}
