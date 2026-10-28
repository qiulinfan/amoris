// Showcase stills and videos are actual engine captures. Videos use native controls without
// autoplay or a separate client-side media controller.

// The theme switch: dark or light, kept under the key the documentation's toggle uses, so both
// follow the same choice.
document.querySelectorAll(".theme-toggle").forEach(function (button) {
  button.addEventListener("click", function () {
    var root = document.documentElement;
    var mode = root.getAttribute("data-bs-theme") === "light" ? "dark" : "light";
    root.setAttribute("data-bs-theme", mode);
    try { localStorage.setItem("mkdocs-colormode", mode); } catch (e) {}
  });
});
