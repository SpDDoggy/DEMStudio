// ETA measures actual rendered work. Preparation never contributes rendering credit.
export function createExportProgress() {
  let started = null;
  let previous = 0;
  let baseline = 0;
  let changedAt = 0;
  let advanceOrigin = 0;
  let history = [];
  const reset = () => { started = null; previous = 0; baseline = 0; changedAt = 0; advanceOrigin = 0; history = []; };
  return (active, progress, now, workProgress) => {
    if (!active) {
      reset();
      return { visible: false, percent: 0, remaining: '' };
    }
    const percent = Math.min(100, Math.max(0, Number(progress) || 0));
    // undefined supports older status producers; explicit null means preparation.
    const legacy = workProgress === undefined;
    const work = legacy ? percent : typeof workProgress === 'number' && Number.isFinite(workProgress)
      ? Math.min(100, Math.max(0, workProgress)) : null;
    if (work === null) reset();
    else {
      if (started === null || work < previous) {
        started = now; previous = work; baseline = work; changedAt = now; advanceOrigin = now; history = [{ time: now, work }];
      }
      if (work > previous && now > changedAt) {
        advanceOrigin = changedAt;
        previous = work; changedAt = now;
      }
      // Keep the point straddling the one-minute window for sparse polling.
      if (history.at(-1)?.time !== now) {
        if (history.length > 1 && history.at(-1).work === work && history.at(-2).work === work) history.at(-1).time = now;
        else history.push({ time: now, work });
      }
      const keepFrom = Math.min(now - 60000, advanceOrigin);
      while (history.length > 2 && history[1].time <= keepFrom) history.shift();
    }
    let remaining = '正在估算…';
    if (percent >= 100) remaining = '导出完成';
    else if (work >= 100 || legacy && percent >= 99) remaining = '正在完成…';
    else if (work !== null && work > baseline && now - started >= 2000 &&
      (baseline >= 95 || now - started >= 120000 || work - baseline >= 25 && now - started >= 10000)) {
      // Top-to-bottom exports can start with cheap transparent padding. Wait for
      // a useful work span or two minutes, then use both long and recent rates.
      const first = history[0], second = history[1];
      // Extend a stalled/sparsely polled window to its last complete work
      // interval: discarding that interval would make the rate fall to zero
      // and an average-rate fallback would suddenly become optimistic again.
      const windowStart = Math.max(started, Math.min(now - 60000, advanceOrigin));
      const startWork = second && second.time > first.time && windowStart > first.time
        ? first.work + (second.work - first.work) * (windowStart - first.time) / (second.time - first.time)
        : first.work;
      const recentRate = (work - startWork) / Math.max(1, now - windowStart);
      const conservativeRate = Math.min(recentRate, (work - baseline) / Math.max(1, now - started));
      const minutes = Math.ceil((100 - work) / conservativeRate / 60000 - 1e-9);
      remaining = minutes <= 1 ? '预计剩余不足 1 分钟' : `预计剩余 ${minutes} 分钟`;
    }
    return { visible: true, percent, remaining };
  };
}
