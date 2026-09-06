(() => {
  const shell = document.querySelector('.shell');
  if (!shell) return;

  const style = document.createElement('style');
  style.textContent = '.chaos-panel{margin-top:16px}.chaos-values{display:grid;grid-template-columns:repeat(3,1fr);gap:8px 18px;color:var(--muted)}.chaos-values strong{color:var(--text);font-variant-numeric:tabular-nums}@media(max-width:850px){.chaos-values{grid-template-columns:repeat(2,1fr)}}';
  document.head.append(style);

  const panel = document.createElement('section');
  panel.className = 'card chaos-panel';
  panel.innerHTML = '<h2>Chaos analysis</h2><div class="chaos-values" id="chaos-values">Loading…</div>';
  shell.append(panel);

  const values = panel.querySelector('#chaos-values');
  const token = new URLSearchParams(location.hash.slice(1)).get('token') || '';

  const number = (value, digits = 3) => Number.isFinite(value) ? value.toFixed(digits) : '—';
  const render = chaos => {
    if (!chaos || !chaos.enabled) {
      values.textContent = 'Disabled — set chaos_enabled = 1 in tuned-main.conf to collect bounded advisory telemetry.';
      return;
    }
    values.innerHTML = [
      ['Lorenz', number(chaos.lorenz_activity)],
      ['Rössler', number(chaos.rossler_activity)],
      ['Logistic', number(chaos.logistic_state)],
      ['Mandelbrot', number(chaos.mandelbrot_complexity)],
      ['Lyapunov', number(chaos.lyapunov_exponent)],
      ['Duffing', number(chaos.duffing_energy)],
      ['Stability', number(chaos.stability)],
      ['Profile bias', number(chaos.profile_bias)],
      ['Samples', String(chaos.samples)],
    ].map(([name, value]) => `<span>${name}<br><strong>${value}</strong></span>`).join('');
  };

  const update = async () => {
    try {
      const response = await fetch('/api/state', {headers: {'X-Tuned-Token': token}});
      const state = await response.json();
      render(state.telemetry && state.telemetry.chaos);
    } catch (_) {
      values.textContent = 'Chaos telemetry unavailable';
    }
  };

  update();
  setInterval(update, 5000);
})();
