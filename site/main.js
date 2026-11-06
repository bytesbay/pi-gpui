// Small, dependency-free touches: the rotating word, a slow star field,
// the screenshot tour, pointer light on cards and the latest release link.

const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

// Latest release: version in the pill and a direct link to the zip.
fetch("https://api.github.com/repos/bytesbay/pi-gpui/releases/latest")
  .then((response) => (response.ok ? response.json() : Promise.reject()))
  .then((release) => {
    const asset = (release.assets || []).find((a) => a.name.endsWith(".zip"));
    document.querySelectorAll("[data-version]").forEach((el) => {
      el.textContent = `${release.tag_name} is out`;
    });
    if (asset) {
      document.querySelectorAll("[data-download]").forEach((el) => {
        el.href = asset.browser_download_url;
      });
    }
  })
  .catch(() => {});

// "A native Mac app for ___": a katana cuts the word in two, like the
// logo, the halves slide apart and fade, and the next word comes in.
const rotor = document.querySelector("[data-rotor]");
if (rotor && !reduceMotion && "animate" in rotor) {
  // Halves and the streak live next to the text, so changing the text
  // does not remove them mid-flight.
  const stage = rotor.parentElement;
  const words = ["pi", "your agent", "pi sessions"];
  let word = 0;
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const done = (animation) => animation.finished.catch(() => {});

  // The cut rises to the right, from 68% of the height on the left to 34% on the right.
  const LEFT = 0.68;
  const RIGHT = 0.34;

  function piece(text, clip) {
    const element = document.createElement("span");
    element.className = "piece";
    element.setAttribute("aria-hidden", "true");
    element.textContent = text;
    element.style.clipPath = clip;
    stage.append(element);
    return element;
  }

  async function slice() {
    const { width, height } = stage.getBoundingClientRect();
    const text = rotor.textContent;
    const rise = (LEFT - RIGHT) * height;
    const angle = Math.atan2(rise, width);

    // The streak: a thin line of light drawn along the cut, a bit past both ends.
    const streak = document.createElement("span");
    streak.className = "streak";
    const overshoot = 0.12 * width;
    const length = Math.hypot(width, rise) + overshoot * 2;
    streak.style.width = `${length}px`;
    const startX = -overshoot * Math.cos(angle);
    const startY = LEFT * height + overshoot * Math.sin(angle);
    const place = `translate(${startX}px, ${startY}px) rotate(${-angle}rad)`;
    stage.append(streak);
    const draw = streak.animate(
      [
        { transform: `${place} scaleX(0)`, opacity: 1 },
        { transform: `${place} scaleX(1)`, opacity: 1, offset: 0.55 },
        { transform: `${place} scaleX(1)`, opacity: 0 },
      ],
      { duration: 420, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)", fill: "forwards" },
    );

    await sleep(130);
    const top = piece(text, `polygon(0 0, 100% 0, 100% ${RIGHT * 100}%, 0 ${LEFT * 100}%)`);
    const bottom = piece(text, `polygon(0 ${LEFT * 100}%, 100% ${RIGHT * 100}%, 100% 100%, 0 100%)`);
    stage.classList.add("cut");
    // The halves slide apart along the cut, then drop and fade.
    const along = 0.06 * height;
    const fall = {
      duration: 1100,
      easing: "cubic-bezier(0.3, 0.6, 0.3, 1)",
      fill: "forwards",
    };
    const fadeOut = (dx, dy, turn) => [
      { transform: "none", opacity: 1, filter: "blur(0)" },
      { transform: `translate(${dx * 0.35}px, ${dy * 0.15}px) rotate(${turn * 0.3}deg)`, opacity: 1, filter: "blur(0)", offset: 0.2 },
      { transform: `translate(${dx * 0.7}px, ${dy * 0.55}px) rotate(${turn * 0.7}deg)`, opacity: 0.75, filter: "blur(1.5px)", offset: 0.55 },
      { transform: `translate(${dx}px, ${dy}px) rotate(${turn}deg)`, opacity: 0, filter: "blur(6px)" },
    ];
    const flyTop = top.animate(fadeOut(along * 2.2, -0.32 * height, -5), fall);
    const flyBottom = bottom.animate(fadeOut(-along * 1.6, 0.42 * height, 4), { ...fall, delay: 40 });

    await sleep(760);
    word = (word + 1) % words.length;
    rotor.textContent = words[word];
    stage.classList.remove("cut");
    rotor.animate(
      [
        { opacity: 0, filter: "blur(8px)", transform: "translateY(0.06em) scale(0.98)" },
        { opacity: 1, filter: "blur(0)", transform: "none" },
      ],
      { duration: 700, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" },
    );
    await Promise.all([done(draw), done(flyTop), done(flyBottom)]);
    streak.remove();
    top.remove();
    bottom.remove();
  }

  (async function loop() {
    for (;;) {
      await sleep(word === 0 ? 4200 : 2400);
      await slice();
    }
  })();
}

// Star field: a few hundred dots drifting slowly, a little parallax.
const canvas = document.getElementById("field");
if (canvas && !reduceMotion) {
  const ctx = canvas.getContext("2d");
  let width = 0;
  let height = 0;
  let dots = [];
  let pointerX = 0;
  let pointerY = 0;
  const ratio = Math.min(window.devicePixelRatio || 1, 2);

  function resize() {
    width = window.innerWidth;
    height = window.innerHeight;
    canvas.width = width * ratio;
    canvas.height = height * ratio;
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    const count = Math.round((width * height) / 9000);
    dots = Array.from({ length: count }, () => ({
      x: Math.random() * width,
      y: Math.random() * height,
      z: Math.random() * 0.9 + 0.1,
      warm: Math.random() < 0.12,
      phase: Math.random() * Math.PI * 2,
    }));
  }

  window.addEventListener("resize", resize);
  window.addEventListener("pointermove", (event) => {
    pointerX = event.clientX / width - 0.5;
    pointerY = event.clientY / height - 0.5;
  });
  resize();

  let last = performance.now();
  function frame(now) {
    const dt = Math.min(now - last, 50) / 1000;
    last = now;
    ctx.clearRect(0, 0, width, height);
    const scroll = window.scrollY * 0.05;
    for (const dot of dots) {
      dot.y -= dt * 6 * dot.z;
      dot.x += dt * 2 * dot.z;
      if (dot.y < -4) dot.y = height + 4;
      if (dot.x > width + 4) dot.x = -4;
      const twinkle = 0.55 + 0.45 * Math.sin(now / 1400 + dot.phase);
      const x = dot.x - pointerX * 22 * dot.z;
      const y = ((dot.y - pointerY * 22 * dot.z - scroll * dot.z) % height + height) % height;
      ctx.globalAlpha = (0.08 + dot.z * 0.35) * twinkle;
      ctx.fillStyle = dot.warm ? "#e6b450" : "#bfbdb6";
      ctx.fillRect(x, y, dot.z * 1.6, dot.z * 1.6);
    }
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
}

// Pointer light on feature cards.
document.querySelectorAll(".card").forEach((card) => {
  card.addEventListener("pointermove", (event) => {
    const box = card.getBoundingClientRect();
    card.style.setProperty("--mx", `${event.clientX - box.left}px`);
    card.style.setProperty("--my", `${event.clientY - box.top}px`);
  });
  card.addEventListener("pointerleave", () => {
    card.style.removeProperty("--mx");
    card.style.removeProperty("--my");
  });
});

// Screenshot tour: tabs switch the picture and advance on their own.
const tour = document.querySelector(".tour");
if (tour) {
  const tabs = [...tour.querySelectorAll("[data-tab]")];
  const figures = [...tour.querySelectorAll("[data-gallery] figure")];
  const interval = 6000;
  tour.style.setProperty("--tour-ms", `${interval}ms`);
  let current = 0;
  let timer = null;
  let paused = false;

  function show(index) {
    current = index;
    tabs.forEach((tab, i) => tab.setAttribute("aria-selected", String(i === index)));
    figures.forEach((figure, i) => figure.classList.toggle("active", i === index));
    // Restart the progress bar on the active tab.
    const active = tabs[index];
    active.style.animation = "none";
    void active.offsetWidth;
    active.style.animation = "";
  }
  function schedule() {
    clearInterval(timer);
    if (reduceMotion) return;
    timer = setInterval(() => {
      if (!paused) show((current + 1) % tabs.length);
    }, interval);
  }
  tabs.forEach((tab, i) =>
    tab.addEventListener("click", () => {
      show(i);
      schedule();
    }),
  );
  const gallery = tour.querySelector("[data-gallery]");
  gallery.addEventListener("pointerenter", () => {
    paused = true;
    tour.classList.add("paused");
  });
  gallery.addEventListener("pointerleave", () => {
    paused = false;
    tour.classList.remove("paused");
  });
  schedule();
}

// Reveal sections as they scroll in.
const revealed = document.querySelectorAll(".reveal, .section");
if ("IntersectionObserver" in window && !reduceMotion) {
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (entry.isIntersecting) {
          entry.target.classList.add("in");
          observer.unobserve(entry.target);
        }
      }
    },
    { rootMargin: "0px 0px -10% 0px" },
  );
  revealed.forEach((el) => {
    el.classList.add("reveal");
    observer.observe(el);
  });
}
