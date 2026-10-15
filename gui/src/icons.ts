// Icons. The strings below are fixed constants in this file, so `innerHTML` is safe here.

export type IconName = "pause" | "play" | "stop" | "check" | "warn" | "info" | "error" | "lock" | "scan" | "rewrite" | "spinner";

const STROKE = 'fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"';

const SHAPES: Record<IconName, string> = {
  pause: '<rect x="6" y="5" width="4" height="14" rx="1"/><rect x="14" y="5" width="4" height="14" rx="1"/>',
  play: '<path d="M8 5v14l11-7z"/>',
  stop: '<rect x="6" y="6" width="12" height="12" rx="2"/>',
  check: `<path ${STROKE} stroke-width="2.4" d="M5 13l4 4L19 7"/>`,
  warn: `<path ${STROKE} d="M12 3l10 18H2z"/><path ${STROKE} d="M12 10v4"/><path ${STROKE} d="M12 17.5v.01"/>`,
  info: `<circle ${STROKE} cx="12" cy="12" r="9"/><path ${STROKE} d="M12 11v5"/><path ${STROKE} d="M12 7.5v.01"/>`,
  error: `<circle ${STROKE} cx="12" cy="12" r="9"/><path ${STROKE} d="M9 9l6 6M15 9l-6 6"/>`,
  lock: `<rect ${STROKE} x="5" y="11" width="14" height="9" rx="2"/><path ${STROKE} d="M8 11V8a4 4 0 018 0v3"/>`,
  scan: `<circle ${STROKE} cx="11" cy="11" r="6.5"/><path ${STROKE} d="M20 20l-4.2-4.2"/>`,
  rewrite: `<path ${STROKE} d="M4 12a8 8 0 0113.7-5.6L20 8"/><path ${STROKE} d="M20 4v4h-4"/><path ${STROKE} d="M20 12a8 8 0 01-13.7 5.6L4 16"/><path ${STROKE} d="M4 20v-4h4"/>`,
  spinner: `<circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-dasharray="28 30" fill="none" class="spin"/>`,
};

export function setIcon(host: HTMLElement, name: IconName, size = 16): void {
  host.innerHTML = `<svg viewBox="0 0 24 24" width="${size}" height="${size}" fill="currentColor" aria-hidden="true">${SHAPES[name]}</svg>`;
}

