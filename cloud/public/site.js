fetch("/api/release")
  .then((r) => (r.ok ? r.json() : null))
  .then((r) => {
    if (r && typeof r.version === "string" && /^[0-9]+(\.[0-9]+){0,3}$/.test(r.version)) {
      document.getElementById("dl-version").textContent = "Version " + r.version;
    }
  })
  .catch(() => {});
