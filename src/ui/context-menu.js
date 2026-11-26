export function menuPosition({ x, y, width, height, viewportWidth, viewportHeight, parent = null, margin = 8 }) {
  let left = parent ? parent.right + 3 : x;
  if (parent && left + width > viewportWidth - margin) left = parent.left - width - 3;
  return {
    left: Math.max(margin, Math.min(left, viewportWidth - width - margin)),
    top: Math.max(margin, Math.min(parent ? parent.top : y, viewportHeight - height - margin)),
  };
}

export function createContextMenu({ onError = () => {}, onChange = () => {}, document: doc = document } = {}) {
  const win = doc.defaultView;
  const root = doc.createElement('div');
  root.className = 'app-context-root'; root.hidden = true;
  root.setAttribute('data-native-overlay', '');
  doc.body.append(root);
  let panels = [], restoreTarget = null, dialog = null;
  const dialogClosed = () => close(false);

  function close(restore = true) {
    if (root.hidden) return;
    dialog?.removeEventListener('close', dialogClosed);
    dialog = null;
    root.hidden = true; root.replaceChildren(); panels = [];
    if (restore && restoreTarget?.isConnected) restoreTarget.focus?.({ preventScroll: true });
    restoreTarget = null;
    onChange();
  }
  function select(panel, row, focus = true) {
    panel.rows.forEach(candidate => { candidate.tabIndex = candidate === row ? 0 : -1; });
    panel.active = row;
    if (focus) row?.focus({ preventScroll: true });
  }
  function removeChildren() {
    if (panels.length < 2) return;
    panels[1].owner.setAttribute('aria-expanded', 'false');
    panels.pop().element.remove(); onChange();
  }
  function place(panel, position) {
    const element = panel.element;
    element.style.maxWidth = `${Math.max(1, win.innerWidth - 16)}px`;
    element.style.maxHeight = `${Math.max(1, win.innerHeight - 16)}px`;
    const bounds = element.getBoundingClientRect();
    const point = menuPosition({ ...position, width: bounds.width, height: bounds.height, viewportWidth: win.innerWidth, viewportHeight: win.innerHeight });
    element.style.left = `${point.left}px`; element.style.top = `${point.top}px`;
    onChange();
  }
  function openChildren(panel, row, focus = false) {
    if (panel !== panels[0] || !row._menuItem.children?.length || row._menuItem.disabled) return;
    if (panels[1]?.owner === row) { if (focus) select(panels[1], panels[1].rows[0]); return; }
    removeChildren();
    const child = makePanel(row._menuItem.children, row);
    row.setAttribute('aria-expanded', 'true');
    place(child, { parent: row.getBoundingClientRect() });
    if (focus) select(child, child.rows[0]);
  }
  async function activate(panel, row) {
    const item = row?._menuItem;
    if (!item || item.disabled) return;
    if (item.children?.length && panel === panels[0]) { openChildren(panel, row, true); return; }
    close();
    try { await item.action?.(); } catch (error) { onError(error); }
  }
  function makePanel(items, owner = null) {
    const element = doc.createElement('div');
    element.className = 'app-context-menu'; element.setAttribute('role', 'menu');
    element.setAttribute('data-native-overlay', '');
    element.setAttribute('aria-label', owner?._menuItem.label || '操作');
    const panel = { element, owner, rows: [], active: null };
    panels.push(panel); root.append(element);
    for (const item of items) {
      if (item.separator || item.type === 'separator') {
        const separator = doc.createElement('div'); separator.className = 'app-context-separator'; separator.setAttribute('role', 'separator'); element.append(separator); continue;
      }
      const row = doc.createElement('button'); row.type = 'button'; row.className = 'app-context-item'; row.tabIndex = -1;
      row._menuItem = item;
      row.setAttribute('role', typeof item.checked === 'boolean' ? 'menuitemcheckbox' : 'menuitem');
      if (typeof item.checked === 'boolean') row.setAttribute('aria-checked', String(item.checked));
      row.setAttribute('aria-disabled', String(Boolean(item.disabled)));
      if (item.children?.length && !owner) { row.setAttribute('aria-haspopup', 'menu'); row.setAttribute('aria-expanded', 'false'); }
      for (const [className, value] of [['mark', item.checked ? '✓' : ''], ['label', item.label], ['shortcut', item.shortcut || ''], ['arrow', item.children?.length && !owner ? '›' : '']]) {
        const span = doc.createElement('span'); span.className = `app-context-${className}`; span.textContent = value ?? ''; row.append(span);
      }
      row.addEventListener('pointerenter', () => {
        if (item.disabled) { if (!owner) removeChildren(); return; }
        select(panel, row, false);
        if (!owner) { if (item.children?.length) openChildren(panel, row); else removeChildren(); }
      });
      row.addEventListener('click', () => activate(panel, row));
      element.append(row);
      if (!item.disabled) panel.rows.push(row);
    }
    return panel;
  }
  function open(items, { x = 0, y = 0, target } = {}) {
    close(false);
    restoreTarget = target || doc.activeElement;
    dialog = restoreTarget?.closest?.('dialog[open]') || null;
    (dialog || doc.body).append(root);
    dialog?.addEventListener('close', dialogClosed);
    root.hidden = false;
    const panel = makePanel(items);
    place(panel, { x, y });
    select(panel, panel.rows[0]);
    if (!panel.rows.length) { panel.element.tabIndex = -1; panel.element.focus(); }
  }
  const keydown = event => {
    if (root.hidden) return;
    const panel = panels.find(candidate => candidate.element.contains(doc.activeElement)) || panels.at(-1);
    const keys = ['ArrowDown', 'ArrowUp', 'ArrowRight', 'ArrowLeft', 'Home', 'End', 'Enter', ' ', 'Escape', 'Tab'];
    if (!keys.includes(event.key)) return;
    event.preventDefault(); event.stopPropagation();
    if (event.key === 'Escape' || event.key === 'Tab') { close(); return; }
    if (event.key === 'ArrowLeft' && panel.owner) { const owner = panel.owner; removeChildren(); select(panels[0], owner); return; }
    if (event.key === 'ArrowLeft') return;
    if (event.key === 'ArrowRight') { if (panel.active) openChildren(panel, panel.active, true); return; }
    if (event.key === 'Enter' || event.key === ' ') { activate(panel, panel.active); return; }
    if (!panel.rows.length) return;
    let index = panel.rows.indexOf(panel.active);
    if (event.key === 'Home') index = 0;
    else if (event.key === 'End') index = panel.rows.length - 1;
    else index = (index + (event.key === 'ArrowUp' ? -1 : 1) + panel.rows.length) % panel.rows.length;
    if (panel === panels[0]) removeChildren();
    select(panel, panel.rows[index]);
  };
  doc.addEventListener('keydown', keydown, true);
  doc.addEventListener('pointerdown', event => { if (!root.hidden && !root.contains(event.target)) close(false); }, true);
  doc.addEventListener('scroll', event => { if (!root.hidden && !root.contains(event.target)) close(false); }, true);
  root.addEventListener('contextmenu', event => event.preventDefault());
  win.addEventListener('blur', () => close(false));
  win.addEventListener('resize', () => close(false));
  return { open, close, get isOpen() { return !root.hidden; } };
}
