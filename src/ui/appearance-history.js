// History owns small immutable snapshots, never image pixels. Asset restoration is
// awaited before moving the cursor; restore must leave the live state intact on failure.
export function createAppearanceHistory({ capture, restore, isLocked = () => false, onChange = () => {}, limit = 64 }) {
  if (typeof capture !== 'function' || typeof restore !== 'function') throw new TypeError('capture/restore required');
  if (!Number.isInteger(limit) || limit < 2) throw new RangeError('limit must be >= 2');
  const copy = value => structuredClone(value);
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  let entries = [copy(capture())], cursor = 0, gesture = false, busy = false;
  const state = () => ({ canUndo: !busy && !isLocked() && cursor > 0, canRedo: !busy && !isLocked() && cursor < entries.length - 1, busy, length: entries.length, cursor, gesture });
  const notify = () => onChange(state());
  function append(snapshot) {
    if (same(entries[cursor], snapshot)) return false;
    entries.splice(cursor + 1);
    entries.push(copy(snapshot));
    if (entries.length > limit) entries.splice(0, entries.length - limit);
    cursor = entries.length - 1;
    notify();
    return true;
  }
  function record() {
    if (busy || isLocked() || gesture) return false;
    return append(capture());
  }
  function begin() {
    if (busy || isLocked() || gesture) return false;
    // Capture any preceding discrete edit before coalescing the next interaction.
    append(capture());
    gesture = true;
    return true;
  }
  function end() {
    if (!gesture || busy) return false;
    gesture = false;
    // Once begun, close the transaction even if an external busy lock just arrived.
    const changed = append(capture());
    if (!changed) notify();
    return changed;
  }
  async function move(direction) {
    if (busy || isLocked()) return false;
    end();
    // A caller may have applied a synchronous edit before its queued record callback.
    append(capture());
    const target = cursor + direction;
    if (target < 0 || target >= entries.length) return false;
    busy = true; notify();
    try {
      await restore(copy(entries[target]), copy(entries[cursor]));
      cursor = target;
      return true;
    } finally { busy = false; notify(); }
  }
  function clear() {
    if (busy) return false;
    entries = [copy(capture())]; cursor = 0; gesture = false; notify();
    return true;
  }
  return { record, begin, end, undo: () => move(-1), redo: () => move(1), clear, state,
    // A renderer cache can retain exactly these asset IDs and release unreachable ones.
    snapshots: () => entries.map(copy), refresh: notify };
}
