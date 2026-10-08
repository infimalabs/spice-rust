const concepts = [
  { key: 'smoke', name: 'Smoked optical', note: 'The lead direction: separate smoked-glass cuts, broad central SEND, crisp white rims.' },
  { key: 'rail', name: 'Split rail · centered', note: 'A slim clear chassis. SEND moves to the middle; the two sliders stay below.' },
  { key: 'frost', name: 'Frosted instrument', note: 'An etched translucent slab with softened rectangular keys. Quiet, precise, symmetric.' },
  { key: 'pearl', name: 'Smoked pearl', note: 'Smoked optical with a restrained pearl sheen. One aligned rim, no split-color halo.' },
  { key: 'satin', name: 'Satin graphite', note: 'A nearby instrument direction: softly diffused dark glass, broad faces, fine etched edges.' },
  { key: 'edge', name: 'Edge-lit rail', note: 'A lighter split-rail hybrid. Thin optical borders and a clear, symmetric central action key.' },
];
const speechModes = ['Quiet', 'Brief', 'Narrate'];
const allocationModes = ['Steer', 'Auto', 'Drain'];
const template = document.querySelector('#panel-template');
const studies = document.querySelector('#studies');

for (const [index, concept] of concepts.entries()) {
  const study = document.createElement('article');
  study.className = `study ${concept.key}`;
  study.innerHTML = `<div class="study-head"><h2><span class="number">0${index + 1}</span>${concept.name}</h2><p>${concept.note}</p></div><div class="pair"></div><div class="state" role="status">Local interaction preview · no commands sent</div>`;
  for (const theme of ['dark', 'light']) {
    const scene = document.createElement('div');
    scene.className = `scene ${theme}`;
    scene.setAttribute('aria-label', `${concept.name}, ${theme} background`);
    const mount = document.createElement('div');
    mount.className = 'mount';
    mount.append(template.content.cloneNode(true));
    scene.append(mount);
    const caption = document.createElement('span');
    caption.className = 'mode-label';
    caption.textContent = theme;
    scene.append(caption);
    study.querySelector('.pair').append(scene);
  }
  studies.append(study);
}

for (const panel of document.querySelectorAll('.panel')) {
  const status = panel.closest('.study').querySelector('.state');
  const turbo = panel.querySelector('.turbo');
  const menu = panel.querySelector('.menu');
  const popover = panel.querySelector('.menu-popover');
  turbo.addEventListener('click', () => {
    const engaged = turbo.getAttribute('aria-pressed') !== 'true';
    turbo.setAttribute('aria-pressed', String(engaged));
    turbo.setAttribute('aria-label', engaged ? 'Disable turbo' : 'Enable turbo');
    turbo.querySelector('span').textContent = engaged ? 'TURBO' : 'BASIC';
    status.textContent = `${engaged ? 'TURBO' : 'BASIC'} · local state only`;
  });
  menu.addEventListener('click', () => {
    popover.hidden = !popover.hidden;
    menu.setAttribute('aria-expanded', String(!popover.hidden));
    status.textContent = popover.hidden ? 'Team menu closed' : 'Team menu · visual preview';
  });
  panel.addEventListener('keydown', event => {
    if (event.key === 'Escape' && !popover.hidden) {
      popover.hidden = true;
      menu.setAttribute('aria-expanded', 'false');
      menu.focus();
    }
  });
  const send = panel.querySelector('.send');
  send.addEventListener('click', () => {
    status.textContent = 'SEND pressed · preview only, nothing sent';
    send.classList.add('pressed');
    setTimeout(() => send.classList.remove('pressed'), 180);
  });
  for (const [selector, modes] of [['.speech', speechModes], ['.life', allocationModes]]) {
    const slider = panel.querySelector(selector);
    const input = slider.querySelector('input');
    input.addEventListener('input', () => {
      const mode = modes[Number(input.value)];
      slider.style.setProperty('--position', Number(input.value) / 2);
      slider.querySelector('.flyer > span').textContent = mode.toUpperCase();
      input.setAttribute('aria-valuetext', mode);
      status.textContent = `${mode} · local slider state`;
    });
  }
}

for (const button of document.querySelectorAll('[data-scale]')) {
  button.addEventListener('click', () => {
    document.documentElement.style.setProperty('--scale', button.dataset.scale);
    for (const other of document.querySelectorAll('[data-scale]')) {
      other.setAttribute('aria-pressed', String(other === button));
    }
  });
}

document.querySelector('#rim-accent').addEventListener('change', event => {
  const color = event.target.value;
  document.documentElement.classList.toggle('accented', Boolean(color));
  document.documentElement.style.setProperty('--agent-accent', color);
});
