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

// On Linux the install command is the main button, not the Mac download.
if (/Linux/.test(navigator.userAgent) && !/Android/.test(navigator.userAgent)) {
  document.querySelectorAll("[data-os]").forEach((el) => {
    el.classList.toggle("btn-ghost", el.dataset.os !== "linux");
  });
  document.querySelectorAll(".nav [data-download]").forEach((el) => {
    el.removeAttribute("data-download");
    el.href = "#linux";
    el.textContent = "Install";
  });
}

// "A native app for ___": a katana cuts the word in two, like the logo.
// The blade flashes through, the halves hold for a beat, then fly apart
// with sparks, and the next word lands letter by letter. A click on the
// word cuts it right away.
const rotor = document.querySelector("[data-rotor]");
if (rotor && !reduceMotion && "animate" in rotor) {
  // Everything that moves lives next to the text and is built up front on
  // one clock, so the real word can change under it at once.
  const stage = rotor.parentElement;
  const words = ["pi", "your agent", "pi sessions"];
  // Where the cut crosses the left and the right edge, as a share of the
  // height. The first rises to the right like the logo, the second is the
  // stroke back down.
  const cuts = [[0.68, 0.34], [0.36, 0.7]];
  const HIT = 90; // ms until the blade is through and the word reacts
  const HOLD = 130; // the cut word hangs still before the halves let go
  const LAND = 600; // the next word starts to arrive
  let word = 0;
  let stroke = 0;
  const rand = (min, max) => min + Math.random() * (max - min);

  function layer(className, parent = stage) {
    const element = document.createElement("span");
    element.className = className;
    element.setAttribute("aria-hidden", "true");
    parent.append(element);
    return element;
  }

  function slice() {
    const { width, height } = stage.getBoundingClientRect();
    const em = parseFloat(getComputedStyle(stage).fontSize);
    const accent = getComputedStyle(stage).color;
    const [a, b] = cuts[stroke++ % cuts.length];
    const drop = (b - a) * height; // negative when the cut rises
    const length = Math.hypot(width, drop);
    const ux = width / length; // unit vector along the blade
    const uy = drop / length;
    const text = rotor.textContent;
    const layers = [];
    const running = [];
    const play = (element, frames, options) => {
      running.push(element.animate(frames, { fill: "both", ...options }));
    };

    // The blade: a needle of light that shoots along the cut, past both
    // ends, then its tail catches up with the tip.
    const lead = 0.25 * em;
    const reach = length + lead + 0.45 * em;
    const blade = layer("streak");
    blade.style.width = `${reach}px`;
    blade.style.transform = `translate(${-lead * ux}px, ${a * height - lead * uy}px) rotate(${Math.atan2(drop, width)}rad)`;
    // The glow is on the outer element, so stretching the light inside
    // does not squash it.
    play(
      layer("light", blade),
      [
        { transform: "translateX(0px) scaleX(0)", opacity: 1, easing: "cubic-bezier(0.5, 0, 0.2, 1)" },
        { transform: "translateX(0px) scaleX(1)", opacity: 1, offset: 0.45, easing: "cubic-bezier(0.5, 0, 0.9, 0.6)" },
        { transform: `translateX(${reach}px) scaleX(0)`, opacity: 0.5 },
      ],
      { duration: 300 },
    );
    layers.push(blade);

    // The halves are padded around the ink; the cut runs through the line
    // box, so shift it by the padding. The pad edges extend the cut line.
    const padding = getComputedStyle(rotor);
    const pad = parseFloat(padding.paddingTop);
    const side = parseFloat(padding.paddingLeft);
    const atLeft = pad + a * height - (side * drop) / width;
    const atRight = pad + b * height + (side * drop) / width;
    const hot = 0.05 * em; // the strip along the cut that glows white
    const gravity = 7 * em;

    // A half slips a little along the cut and hangs, then it is thrown:
    // steady speed sideways, gravity down, a steady spin.
    function half(clip, glow, middle, { slip, vx, vy, spin, wait }) {
      const piece = layer("piece");
      piece.style.transformOrigin = `50% ${pad + middle * height}px`;
      const ink = layer("ink", piece);
      ink.textContent = text;
      ink.style.clipPath = clip;
      const edge = layer("ink hot", piece);
      edge.textContent = text;
      edge.style.clipPath = glow;
      layers.push(piece);

      const air = 480;
      const total = HOLD + wait + air;
      const frames = [
        { transform: "translate(0px, 0px) rotate(0deg)", opacity: 1, easing: "cubic-bezier(0.1, 0.9, 0.2, 1)" },
        { transform: `translate(${slip * ux}px, ${slip * uy}px) rotate(0deg)`, opacity: 1, offset: (HOLD + wait) / total },
      ];
      for (let step = 1; step <= 10; step++) {
        const part = step / 10;
        const t = (part * air) / 1000;
        const x = slip * ux + vx * t;
        const y = slip * uy + vy * t + (gravity * t * t) / 2;
        frames.push({
          transform: `translate(${x}px, ${y}px) rotate(${spin * t}deg)`,
          opacity: Math.max(0, 1 - Math.max(0, (part - 0.15) / 0.7) ** 1.3),
          offset: (HOLD + wait + part * air) / total,
        });
      }
      play(piece, frames, { duration: total, delay: HIT });
      // The word flashes white when it is hit, and the cut edge cools last.
      play(ink, [{ color: "#fff6dc" }, { color: accent }], { duration: 280, delay: HIT, fill: "forwards" });
      play(edge, [{ opacity: 0 }, { opacity: 1, offset: 0.12 }, { opacity: 0 }], { duration: 560, delay: HIT - 30 });
    }

    const mid = (a + b) / 2;
    // Long words turn slower, or their ends would swing across the page.
    // The halves open like jaws on the side where the cut ends high.
    const turn = -Math.sign(drop) * Math.min(1, (1.2 * em) / width);
    half(
      `polygon(0 0, 100% 0, 100% ${atRight + 0.5}px, 0 ${atLeft + 0.5}px)`,
      `polygon(0 ${atLeft - hot}px, 100% ${atRight - hot}px, 100% ${atRight + 0.5}px, 0 ${atLeft + 0.5}px)`,
      mid / 2,
      { slip: 0.04 * em, vx: 2 * em * ux, vy: 2 * em * uy - 1.2 * em, spin: 34 * turn, wait: 0 },
    );
    half(
      `polygon(0 ${atLeft}px, 100% ${atRight}px, 100% 100%, 0 100%)`,
      `polygon(0 ${atLeft}px, 100% ${atRight}px, 100% ${atRight + hot}px, 0 ${atLeft + hot}px)`,
      (1 + mid) / 2,
      { slip: -0.015 * em, vx: -0.6 * em * ux, vy: -0.6 * em * uy + 0.3 * em, spin: -20 * turn, wait: 40 },
    );

    // The hit shakes the word across the cut.
    const jolt = 0.028 * em;
    play(
      stage,
      [0, 1, -0.6, 0.3, -0.12, 0].map((k) => ({ transform: `translate(${-uy * jolt * k}px, ${ux * jolt * k}px)` })),
      { duration: 260, delay: HIT - 20, easing: "ease-out", fill: "none" },
    );

    // Sparks leave the cut as the blade passes, mostly the way it goes and
    // most of them near the end of the stroke.
    const sparks = Math.round(Math.min(22, 10 + (width / em) * 2));
    for (let i = 0; i < sparks; i++) {
      const at = 1 - 0.9 * Math.random() ** 2;
      const speed = rand(1.5, 6) * em;
      const across = rand(-2, 2) * em;
      const vx = ux * speed - uy * across;
      const vy = uy * speed + ux * across - rand(0, 0.8) * em;
      const pull = 9 * em;
      const life = rand(240, 520);
      const spark = layer("spark");
      spark.style.width = `${rand(0.1, 0.3) * em}px`;
      const frames = [];
      for (let step = 0; step <= 6; step++) {
        const part = step / 6;
        const t = (part * life) / 1000;
        const x = at * width + vx * t;
        const y = a * height + at * drop + vy * t + (pull * t * t) / 2;
        const heading = Math.atan2(vy + pull * t, vx);
        frames.push({
          transform: `translate(${x}px, ${y}px) rotate(${heading}rad) scaleX(${1 - 0.75 * part})`,
          opacity: 1 - part ** 3,
        });
      }
      play(spark, frames, { duration: life, delay: 25 + at * 110, fill: "forwards" });
      layers.push(spark);
    }

    // The next word takes the place at once, hidden, and its letters jump
    // in one by one over it, hot at first.
    word = (word + 1) % words.length;
    rotor.textContent = words[word];
    stage.classList.add("cut");
    const fresh = layer("ink", layer("piece"));
    layers.push(fresh.parentElement);
    const letters = [...words[word]];
    const gap = Math.min(46, 300 / letters.length);
    letters.forEach((character, i) => {
      const letter = layer("letter", fresh);
      letter.textContent = character;
      play(
        letter,
        [
          { transform: "translateY(0.42em) rotate(9deg)", opacity: 0, color: "#fff6dc" },
          { opacity: 1, color: "#fff6dc", offset: 0.45 },
          { transform: "translateY(0) rotate(0deg)", opacity: 1, color: accent },
        ],
        { duration: 560, delay: LAND + i * gap, easing: "cubic-bezier(0.2, 1.5, 0.35, 1)" },
      );
    });

    return Promise.all(running.map((animation) => animation.finished.catch(() => {}))).then(() => {
      stage.classList.remove("cut");
      layers.forEach((element) => element.remove());
    });
  }

  let poke = () => {};
  stage.addEventListener("click", () => poke());
  (async function loop() {
    for (;;) {
      await new Promise((resolve) => {
        poke = resolve;
        setTimeout(resolve, word === 0 ? 2200 : 1300);
      });
      if (!document.hidden) await slice();
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
