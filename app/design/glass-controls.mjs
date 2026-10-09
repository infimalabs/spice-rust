const concepts = [
  { key: 'pearl', name: 'Smoked pearl', note: 'Reference-only alternative. Smoked glass with a restrained pearl sheen and one optical edge.' },
  { key: 'frost', name: 'Frosted instrument', note: 'Reference-only alternative. Softly diffused glass in the same sharp-corner panel.' },
  { key: 'flush', name: 'Flush etched glass', note: 'Shipped production panel: flat transmitted glass at rest, a clear tactile depression on press.' },
];
const underglowConcepts = [
  { key: 'baseline', name: 'Shipped baseline', note: 'Untouched Flush groove. Neutral backplane, no colored light.' },
  { key: 'diffuse', name: 'Three-stop bed', note: 'Cool lilac, green, and amber wells remain visible across the groove.' },
  { key: 'tracking', name: 'Active pool', note: 'One concentrated light follows the flyer and changes color at each stop.' },
  { key: 'rails', name: 'Active edge rails', note: 'Fine perimeter lights follow the flyer’s selected lilac, green, or amber stop.' },
];
const speechModes = ['Quiet', 'Speak', 'Narrate'];
const allocationModes = ['Steer', 'Drive', 'Drain'];
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

for (const [index, concept] of underglowConcepts.entries()) {
  const study = document.createElement('article');
  study.className = `study flush underglow-${concept.key}`;
  study.innerHTML = `<div class="study-head"><h2><span class="number">U${index}</span>${concept.name}</h2><p>${concept.note}</p></div><div class="pair"></div><div class="state" role="status">Drag either slider · local light study</div>`;
  for (const theme of ['dark', 'light']) {
    const scene = document.createElement('div');
    scene.className = `scene ${theme}`;
    scene.setAttribute('aria-label', `${concept.name}, ${theme} background`);
    const mount = document.createElement('div');
    mount.className = 'mount';
    const content = template.content.cloneNode(true);
    setPreviewState(content.querySelector('.panel'), 'one');
    mount.append(content);
    scene.append(mount);
    const caption = document.createElement('span');
    caption.className = 'mode-label';
    caption.textContent = theme;
    scene.append(caption);
    study.querySelector('.pair').append(scene);
  }
  document.querySelector('#underglow-studies').append(study);
}
const previewStates = [
  { key: 'off', name: 'Totally off', note: 'No agents running. The whole panel rests; SEND is unavailable.' },
  { key: 'idle', name: 'Running · no text', note: 'The panel is awake. No draft is available, so SEND stays quiet.' },
  { key: 'one', name: 'Text · one agent', note: 'One recipient, one accent rim. SEND is available.' },
  { key: 'many', name: 'Text · N agents', note: 'Four recipients shown. A segmented two-pixel rim, not a multicolored face.' },
];
for (const state of previewStates) {
  const study = document.createElement('article');
  study.className = 'study flush';
  study.innerHTML = `<div class="study-head"><h3>${state.name}</h3><p>${state.note}</p></div><div class="scene dark"><div class="mount"></div><span class="mode-label">Dark</span></div><div class="state" role="status">Local state preview · no commands sent</div>`;
  const content = template.content.cloneNode(true);
  const panel = content.querySelector('.panel');
  setPreviewState(panel, state.key);
  study.querySelector('.mount').append(content);
  document.querySelector('#state-matrix').append(study);
}
for (const panel of studies.querySelectorAll('.panel')) {
  setPreviewState(panel, 'one');
}

for (const concept of concepts) {
  const study = document.createElement('article');
  study.className = `study ${concept.key}`;
  study.innerHTML = `<div class="study-head"><h3>${concept.name}</h3><p>Released / held. Same glass, different contact depth.</p></div><div class="pair"></div><div class="state" role="status">Local press comparison · no commands sent</div>`;
  for (const held of [false, true]) {
    const scene = document.createElement('div');
    scene.className = 'scene dark';
    scene.innerHTML = `<div class="mount"></div><span class="mode-label">${held ? 'Held down' : 'Released'}</span>`;
    const content = template.content.cloneNode(true);
    const panel = content.querySelector('.panel');
    setPreviewState(panel, 'one');
    panel.dataset.held = String(held);
    scene.querySelector('.mount').append(content);
    study.querySelector('.pair').append(scene);
  }
  document.querySelector('#press-matrix').append(study);
}

function setPreviewState(panel, key) {
  const ready = key === 'one' || key === 'many';
  panel.dataset.previewState = key;
  const send = panel.querySelector('.send');
  send.disabled = !ready;
  send.setAttribute('aria-label', key === 'off' ? 'Send unavailable: no agents running'
    : key === 'idle' ? 'Send unavailable: no text'
      : `Preview send to ${key === 'one' ? '1 agent' : '4 agents'}`);
  send.title = send.getAttribute('aria-label');
  panel.querySelector('.recipient-count').textContent = ready ? (key === 'one' ? '1' : '4') : '';
}

document.querySelector('#panel-readiness').addEventListener('change', event => {
  for (const panel of studies.querySelectorAll('.panel')) {
    setPreviewState(panel, event.target.value);
  }
});
document.querySelector('#state-finish').addEventListener('change', event => {
  for (const study of document.querySelectorAll('#state-matrix .study')) {
    study.className = `study ${event.target.value}`;
  }
});
document.querySelector('#state-background').addEventListener('change', event => {
  for (const scene of document.querySelectorAll('#state-matrix .scene')) {
    scene.className = `scene ${event.target.value}`;
    scene.querySelector('.mode-label').textContent = event.target.value;
  }
});

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
    // Native :active owns depression; release restores the optical surface.
  });
  for (const [selector, modes] of [['.speech', speechModes], ['.life', allocationModes]]) {
    const slider = panel.querySelector(selector);
    const input = slider.querySelector('input');
    slider.dataset.step = input.value;
    input.addEventListener('input', () => {
      const mode = modes[Number(input.value)];
      slider.style.setProperty('--position', Number(input.value) / 2);
      slider.dataset.step = input.value;
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
