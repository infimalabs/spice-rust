const concepts = [
  { key: 'lens', name: 'Clear lens', note: 'Thick transparent rims. Floating controls. Closest to the hamburger’s direction.' },
  { key: 'frost', name: 'Frosted instrument', note: 'One etched glass chassis. Squared lenses. Quiet and precise.' },
  { key: 'smoke', name: 'Smoked optical', note: 'Dark translucent cuts. A broad SEND key. Sharp edge illumination.' },
  { key: 'opal', name: 'Opal stack', note: 'Pearlescent rims. SEND first. Soft color dispersion without colored paint.' },
  { key: 'droplet', name: 'Liquid droplets', note: 'Rounded volumes with wet highlights. Open substrate. Organic SEND lens.' },
  { key: 'rail', name: 'Split rail', note: 'Sliders between utility and action rows. A clear lower SEND lens.' },
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
