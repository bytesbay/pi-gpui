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

// "A native Mac app for ___": types and erases a few endings.
const rotor = document.querySelector("[data-rotor]");
if (rotor && !reduceMotion) {
  const words = ["pi", "your agent", "pi sessions", "Horse Tinder", "pi"];
  let word = 0;
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  (async function loop() {
    await sleep(2600);
    for (;;) {
      const current = words[word];
      for (let i = current.length; i >= 0; i--) {
        rotor.textContent = current.slice(0, i);
        await sleep(45);
      }
      word = (word + 1) % words.length;
      const next = words[word];
      for (let i = 1; i <= next.length; i++) {
        rotor.textContent = next.slice(0, i);
        await sleep(70 + Math.random() * 60);
      }
      await sleep(word === words.length - 1 ? 5200 : 2200);
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
