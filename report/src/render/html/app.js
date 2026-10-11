// Sort toggles on the paired-bar charts. Optional: with JS off the controls stay hidden and the
// rows keep their server order (by spend). Rows carry their rank under each key in a data- attribute,
// so sorting only reorders existing nodes; rows past the figure's data-top fold behind its expander.
(function () {
  "use strict";
  var figures = document.querySelectorAll("[data-sortable]");
  Array.prototype.forEach.call(figures, function (figure) {
    var control = figure.querySelector(".sort");
    var list = figure.querySelector(".pairs");
    if (!control || !list) {
      return;
    }
    var top = Number(figure.getAttribute("data-top"));
    var buttons = control.querySelectorAll("button");
    control.hidden = false;
    Array.prototype.forEach.call(buttons, function (button) {
      button.addEventListener("click", function () {
        var key = "data-" + button.getAttribute("data-key");
        var rows = Array.prototype.slice.call(list.children);
        rows.sort(function (a, b) {
          return Number(a.getAttribute(key)) - Number(b.getAttribute(key));
        });
        rows.forEach(function (row, index) {
          row.classList.toggle("extra", index >= top);
          list.appendChild(row);
        });
        Array.prototype.forEach.call(buttons, function (other) {
          other.setAttribute("aria-pressed", other === button ? "true" : "false");
        });
      });
    });
  });
})();

// Hover tooltips for any element carrying data-tip. Optional: with JS off, each chart column keeps
// its SVG title as the browser's native tooltip, and every figure is already printed on the page.
// This script builds no display text of its own; it only moves data-tip into the tooltip.
(function () {
  "use strict";
  var targets = document.querySelectorAll("[data-tip]");
  if (!targets.length) {
    return;
  }
  // The custom tooltip replaces the native one, so the SVG titles would only show a second copy.
  Array.prototype.forEach.call(targets, function (el) {
    var title = el.querySelector("title");
    if (title) {
      el.removeChild(title);
    }
  });

  var tip = document.createElement("div");
  tip.className = "tip";
  tip.setAttribute("role", "tooltip");
  tip.hidden = true;
  document.body.appendChild(tip);

  var gap = 14;

  function place(event) {
    var box = tip.getBoundingClientRect();
    var x = event.clientX + gap;
    var y = event.clientY - box.height - gap;
    if (x + box.width > window.innerWidth - gap) {
      x = event.clientX - box.width - gap;
    }
    if (y < gap) {
      y = event.clientY + gap;
    }
    tip.style.left = Math.max(gap, x) + "px";
    tip.style.top = y + "px";
  }

  function owner(node) {
    return node && node.closest ? node.closest("[data-tip]") : null;
  }

  document.addEventListener("pointerover", function (event) {
    var el = owner(event.target);
    if (!el) {
      tip.hidden = true;
      return;
    }
    tip.textContent = el.getAttribute("data-tip");
    tip.hidden = false;
    place(event);
  });

  document.addEventListener("pointermove", function (event) {
    if (!tip.hidden) {
      place(event);
    }
  });

  document.addEventListener("pointerout", function (event) {
    if (!owner(event.relatedTarget)) {
      tip.hidden = true;
    }
  });
})();
