import { $ } from "../dom";
import { type IconName, setIcon } from "../icons";
import type { NoticeKind, Store } from "../store";

const TONE_ICON: Record<NoticeKind, IconName> = {
  ok: "check",
  info: "info",
  warn: "warn",
  error: "error",
};

export function mountNotice(store: Store): void {
  const card = $("notice-card");
  const icon = $("notice-icon");
  const title = $("notice-title");
  const body = $("notice-body");
  const dismiss = $("btn-notice-dismiss");

  dismiss.addEventListener("click", () => {
    store.set({ notice: null });
  });

  store.subscribe((s, prev) => {
    if (!prev || s.notice !== prev.notice) {
      if (!s.notice) {
        card.hidden = true;
        return;
      }
      card.hidden = false;
      card.dataset.tone = s.notice.kind;
      setIcon(icon, TONE_ICON[s.notice.kind], 20);
      title.textContent = s.notice.title;
      body.textContent = s.notice.body ?? "";
      body.hidden = !s.notice.body;
    }
  });
}
