// Runs before module initialization, including while the renderer is starting.
document.addEventListener('contextmenu', event => event.preventDefault(), { capture: true });
