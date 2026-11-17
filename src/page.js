(() => {
  const post = message => window.ipc.postMessage(JSON.stringify(message));
  document.addEventListener("keydown", event => {
    if (event.isTrusted && event.key === "Escape" && !document.fullscreenElement) {
      post({action: "exit-fullscreen"});
    }
  }, true);
  let observer;
  let domains;
  window.__browserNetwork = enabled => {
    observer?.disconnect();
    observer = undefined;
    if (!enabled) return;
    domains = new Map();
    observer = new PerformanceObserver(list => {
      for (const entry of list.getEntries()) {
        const url = new URL(entry.name);
        if (!['http:', 'https:'].includes(url.protocol)) continue;
        const key = url.hostname + (entry.nextHopProtocol ? ' (' + entry.nextHopProtocol + ')' : '');
        if (!domains.has(key) && domains.size >= 100) continue;
        const totals = domains.get(key) ?? [0, 0];
        totals[0] += entry.transferSize;
        totals[1]++;
        domains.set(key, totals);
      }
    });
    observer.observe({type: 'resource', buffered: true});
  };
  window.__browserNetworkSample = () => {
    if (!observer) return;
    post({action: 'network-sample', value: Array.from(domains, ([key, totals]) => [key, ...totals])});
  };
})();
