// Keep the native select as the value/event source; paint every visible state ourselves.
export function enhanceSelects(doc, onLayout = () => {}) {
  const controls = new Map();
  let active = null, serial = 0, typed = '', typedAt = 0;
  const win = doc.defaultView;
  function close(focus = false) {
    if (!active) return;
    const {button, menu} = active;
    active = null;
    menu.remove(); button.setAttribute('aria-expanded', 'false');
    button.removeAttribute('aria-activedescendant');
    if (focus && !button.disabled) button.focus();
    onLayout();
  }
  function highlight(index) {
    if (!active) return;
    active.index = index;
    for (const row of active.menu.children) row.classList.toggle('highlighted', Number(row.dataset.index) === index);
    const row = active.menu.children[index];
    if (row) { active.button.setAttribute('aria-activedescendant', row.id); row.scrollIntoView({block:'nearest'}); }
  }
  function move(delta, edge) {
    const enabled = [...active.select.options].map((option,index) => ({option,index})).filter(({option}) => !option.disabled);
    if (!enabled.length) return;
    const position = enabled.findIndex(({index}) => index === active.index);
    const next = edge === 'first' ? 0 : edge === 'last' ? enabled.length-1 : Math.max(0, Math.min(enabled.length-1, position + delta));
    highlight(enabled[next].index);
  }
  function choose(index) {
    if (!active || active.select.disabled || active.select.options[index]?.disabled) return;
    const select = active.select;
    const changed = select.selectedIndex !== index;
    select.selectedIndex = index;
    close(true); sync();
    if (changed) {
      select.dispatchEvent(new win.Event('input', {bubbles:true}));
      select.dispatchEvent(new win.Event('change', {bubbles:true}));
      sync();
    }
  }
  function open(control) {
    if (control.select.disabled) return;
    close(); typed = '';
    const {select, button} = control;
    const menu = doc.createElement('div');
    menu.className = 'combo-menu'; menu.id = button.getAttribute('aria-controls');
    menu.setAttribute('role','listbox'); menu.setAttribute('aria-label',button.getAttribute('aria-label'));
    menu.setAttribute('data-native-overlay','');
    [...select.options].forEach((option,index) => {
      const row = doc.createElement('div'); row.className = 'combo-option';
      row.id = `${menu.id}-${index}`; row.dataset.index = String(index); row.textContent = option.textContent;
      row.setAttribute('role','option'); row.setAttribute('aria-selected',String(index === select.selectedIndex));
      row.setAttribute('aria-disabled',String(option.disabled));
      row.addEventListener('pointermove', () => { if (!option.disabled) highlight(index); });
      row.addEventListener('pointerdown', event => event.preventDefault());
      row.addEventListener('click', () => choose(index));
      menu.append(row);
    });
    (select.closest('dialog[open]') ?? doc.body).append(menu);
    const rect = button.getBoundingClientRect();
    const available = Math.max(32, win.innerHeight - 16);
    menu.style.maxHeight = `${Math.min(280, available)}px`;
    menu.style.minWidth = `${Math.min(Math.max(rect.width,128),win.innerWidth-16)}px`;
    menu.style.maxWidth = `${win.innerWidth-16}px`;
    const bounds = menu.getBoundingClientRect();
    const below = win.innerHeight - rect.bottom - 12;
    const above = rect.top - 12;
    const up = bounds.height > below && above > below;
    const room = Math.max(32, up ? above : below);
    menu.style.maxHeight = `${Math.min(280,room)}px`;
    const height = Math.min(bounds.height,room);
    menu.style.left = `${Math.max(8,Math.min(rect.right-bounds.width,win.innerWidth-bounds.width-8))}px`;
    menu.style.top = `${Math.max(8, up ? rect.top-height-6 : rect.bottom+6)}px`;
    active = {...control,menu,index:select.selectedIndex};
    button.setAttribute('aria-expanded','true');
    highlight(select.selectedIndex);
    onLayout();
  }
  function keydown(event, control) {
    if (control.select.disabled) return;
    const keys = ['ArrowDown','ArrowUp','Home','End','Enter',' ','Escape'];
    if (event.key === 'Tab') { close(); return; }
    if (event.key === 'Escape') { if (active) { event.preventDefault(); event.stopPropagation(); close(true); } return; }
    if (keys.includes(event.key)) {
      event.preventDefault(); event.stopPropagation();
      const wasOpen = active?.select === control.select;
      if (!wasOpen) open(control);
      if (event.key === 'Home') move(0,'first');
      else if (event.key === 'End') move(0,'last');
      else if (event.key === 'ArrowDown' && wasOpen) move(1);
      else if (event.key === 'ArrowUp' && wasOpen) move(-1);
      else if (wasOpen && (event.key === 'Enter' || event.key === ' ')) choose(active.index);
      return;
    }
    if (event.key.length === 1 && !event.ctrlKey && !event.altKey && !event.metaKey) {
      event.preventDefault();
      if (active?.select !== control.select) open(control);
      const now = Date.now(); typed = now-typedAt > 700 ? event.key : typed+event.key; typedAt=now;
      const index = [...control.select.options].findIndex(option => !option.disabled && option.textContent.toLocaleLowerCase().startsWith(typed.toLocaleLowerCase()));
      if (index >= 0) highlight(index);
    }
  }
  function sync() {
    for (const select of doc.querySelectorAll('select')) {
      if (!controls.has(select)) {
        const wrapper = doc.createElement('span'); wrapper.className = 'combo';
        const button = doc.createElement('button'); button.type='button'; button.className='combo-trigger';
        const text = doc.createElement('span'); text.className='combo-text'; button.append(text);
        const arrow = doc.createElement('span'); arrow.className='combo-arrow'; arrow.setAttribute('aria-hidden','true'); button.append(arrow);
        button.setAttribute('role','combobox'); button.setAttribute('aria-haspopup','listbox');
        button.setAttribute('aria-expanded','false'); button.setAttribute('aria-controls',`combo-list-${++serial}`);
        button.setAttribute('aria-label',select.getAttribute('aria-label') ?? '选择');
        select.before(wrapper); wrapper.append(select,button);
        select.classList.add('combo-native'); select.tabIndex=-1; select.setAttribute('aria-hidden','true');
        const control = {select,button,text}; controls.set(select,control);
        button.addEventListener('click',event => { event.preventDefault(); if (active?.select === select) close(); else open(control); });
        button.addEventListener('keydown',event => keydown(event,control));
        button.addEventListener('blur',()=>{if(active?.select===select)close()});
        // Clicking the surrounding field label forwards a click to its select.
        // Route that path through the same custom list, never the OS picker.
        select.addEventListener('click',event=>{event.preventDefault();button.focus();if(active?.select===select)close();else open(control)});
        select.addEventListener('focus',()=>button.focus());
        select.addEventListener('input',sync); select.addEventListener('change',sync);
      }
      const control = controls.get(select), value = select.selectedOptions[0]?.textContent ?? '';
      if (control.text.textContent !== value) control.text.textContent=value;
      if (control.button.disabled !== select.disabled) control.button.disabled=select.disabled;
      control.button.title=value;
    }
    if (active && (active.select.disabled || !active.select.isConnected || active.select.closest('[hidden]') || active.select.closest('dialog:not([open])'))) close();
  }
  doc.addEventListener('pointerdown',event=>{if(active && !active.button.contains(event.target) && !active.menu.contains(event.target))close()},true);
  doc.addEventListener('scroll',event=>{if(active && !active.menu.contains(event.target))close()},true);
  win.addEventListener('resize',()=>close());
  doc.addEventListener('close',()=>close(),true);
  sync();
  const observer = new win.MutationObserver(sync);
  observer.observe(doc.body,{subtree:true,childList:true,characterData:true,attributes:true,attributeFilter:['disabled','selected','hidden']});
  return {sync,close};
}
