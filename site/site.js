const ambient = document.querySelector(".ambient-video");
const toggle = document.querySelector(".motion-toggle");
const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
if (ambient && toggle) {
  const label = () => {
    toggle.textContent = ambient.paused ? "Play motion" : "Pause motion";
    toggle.setAttribute("aria-label", ambient.paused ? "Play the scene video" : "Pause the scene video");
    toggle.setAttribute("aria-pressed", String(ambient.paused));
  };
  if (reducedMotion.matches) ambient.pause();
  ambient.addEventListener("play", label);
  ambient.addEventListener("pause", label);
  toggle.addEventListener("click", () => {
    if (ambient.paused) ambient.play().catch(() => {});
    else ambient.pause();
  });
  reducedMotion.addEventListener("change", (event) => { if (event.matches) ambient.pause(); });
  label();
}
for (const video of document.querySelectorAll("video[controls]")) {
  video.addEventListener("play", () => {
    for (const other of document.querySelectorAll("video[controls]")) if (other !== video) other.pause();
  });
}
