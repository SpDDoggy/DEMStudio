// ETA uses completed work, never a timer that advances the progress itself.
export function createExportProgress() {
  let started = null;
  let previous = 0;
  let changedAt = 0;
  let rate = null;
  return (active, progress, now) => {
    if (!active) {
      started = null; previous = 0; rate = null;
      return { visible: false, percent: 0, remaining: '' };
    }
    const percent = Math.min(100, Math.max(0, Number(progress) || 0));
    if (started === null || percent < previous) {
      started = now; previous = 0; changedAt = now; rate = null;
    }
    if (percent > previous && now > changedAt) {
      const measured = (percent - previous) / (now - changedAt);
      rate = rate === null ? measured : rate * 0.7 + measured * 0.3;
      previous = percent; changedAt = now;
    }
    let remaining = '正在估算…';
    if (percent >= 100) remaining = '导出完成';
    else if (percent >= 99) remaining = '正在完成…';
    else if (rate !== null && now - started >= 2000) {
      // A stalled sample must not produce an ETA that counts down to zero.
      const conservativeRate = Math.min(rate, percent / Math.max(1, now - started));
      const minutes = Math.ceil((100 - percent) / conservativeRate / 60000 - 1e-9);
      remaining = minutes <= 1 ? '预计剩余不足 1 分钟' : `预计剩余 ${minutes} 分钟`;
    }
    return { visible: true, percent, remaining };
  };
}
