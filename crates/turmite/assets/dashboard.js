// Optional: copy buttons and confirm prompts. Every page works without this file.
document.addEventListener("click", (e) => {
  const copy = e.target.closest("[data-copy]");
  if (copy) {
    e.preventDefault();
    navigator.clipboard.writeText(copy.getAttribute("data-copy")).then(() => {
      const label = copy.textContent;
      copy.textContent = "Copied";
      copy.classList.add("copied");
      setTimeout(() => { copy.textContent = label; copy.classList.remove("copied"); }, 1400);
    });
  }
});
document.addEventListener("submit", (e) => {
  const msg = e.target.getAttribute("data-confirm");
  if (msg && !window.confirm(msg)) e.preventDefault();
});
document.querySelectorAll("[data-copy]").forEach((el) => { el.hidden = false; });
