import mermaid from './mermaid.esm.min.mjs';

// This module runs only in the generation-pinned, allow-scripts-only iframe.
// Its output is XML for an image, never markup admitted into the parent DOM.
const moduleUrl = new URL(import.meta.url);
const parentOrigin = moduleUrl.origin;
const generation = moduleUrl.pathname.split('/').at(-2);
if (!/^[a-f0-9]{64}$/.test(generation)) throw Error('renderer requires an immutable generation');
const protocol = 1;
const sourceLimit = 50000;
const resultLimit = 8 * 1024 * 1024;
const encoder = new TextEncoder();
const secure = [
  'secure', 'securityLevel', 'startOnLoad', 'maxTextSize', 'maxEdges',
  'theme', 'themeVariables', 'themeCSS', 'fontFamily', 'altFontFamily',
  'fontSize', 'htmlLabels', 'dompurifyConfig', 'suppressErrorRendering',
  'legacyMathML', 'forceLegacyMathML', 'deterministicIds',
  'deterministicIDSeed', 'arrowMarkerAbsolute',
];
let busy = false;
let serial = 0;
const renderHost = document.createElement('div');
document.body.appendChild(renderHost);

function isRequest(data) {
  return data && data.type === 'render' && data.protocol === protocol
    && data.generation === generation
    && typeof data.requestId === 'string' && /^[a-z0-9-]{1,64}$/.test(data.requestId)
    && typeof data.channel === 'string' && /^[a-z0-9-]{1,64}$/.test(data.channel)
    && Number.isSafeInteger(data.revision) && data.revision >= 0
    && (data.theme === 'light' || data.theme === 'dark')
    && typeof data.source === 'string' && data.source.length <= sourceLimit;
}

addEventListener('message', async event => {
  if (event.source !== parent || event.origin !== parentOrigin || !isRequest(event.data)) return;
  const { requestId, channel, revision, theme, source } = event.data;
  const envelope = { protocol, generation, requestId, channel, revision, theme };
  const reply = data => parent.postMessage({ ...envelope, ...data }, parentOrigin);
  if (busy) {
    reply({ type: 'error', error: 'renderer busy' });
    return;
  }
  busy = true;
  try {
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: 'strict',
      htmlLabels: true,
      theme: theme === 'dark' ? 'dark' : 'default',
      fontFamily: 'Arial, sans-serif',
      maxTextSize: sourceLimit,
      maxEdges: 500,
      suppressErrorRendering: true,
      legacyMathML: false,
      forceLegacyMathML: false,
      arrowMarkerAbsolute: false,
      secure,
    });
    const rendered = await mermaid.render(`spice-diagram-${++serial}`, source, renderHost);
    // Mermaid serializes HTML namespaces. The opaque browser context repairs
    // them before XML serialization so foreignObject and native MathML survive.
    const inert = new DOMParser().parseFromString(rendered.svg, 'text/html');
    const root = inert.querySelector('svg');
    if (!root || root.namespaceURI !== 'http://www.w3.org/2000/svg') {
      throw Error('renderer did not return SVG');
    }
    const svg = new XMLSerializer().serializeToString(root);
    if (encoder.encode(svg).length > resultLimit) throw Error('SVG exceeds output bound');
    reply({ type: 'rendered', svg });
  } catch (error) {
    reply({ type: 'error', error: String(error).slice(0, 500) });
  } finally {
    renderHost.replaceChildren();
    busy = false;
  }
});
parent.postMessage({ type: 'ready', protocol, generation }, parentOrigin);
