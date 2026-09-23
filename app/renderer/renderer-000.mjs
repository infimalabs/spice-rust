import mermaid from './mermaid.esm.min.mjs';

// This module runs only in the generation-pinned, allow-scripts-only iframe.
// Its output is XML for an image, never markup admitted into the parent DOM.
const moduleUrl = new URL(import.meta.url);
const parentOrigin = moduleUrl.origin;
const generation = moduleUrl.pathname.split('/').at(-2);
if (!/^[a-f0-9]{64}$/.test(generation)) throw Error('renderer requires an immutable generation');
const protocol = 2;
const sourceLimit = 50000;
const resultLimit = 8 * 1024 * 1024;
const nodeLimit = 128;
const coordinateLimit = 1000000;
const nodeIdPattern = /^[A-Za-z][A-Za-z0-9_-]{0,63}$/;
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

function validNodeIds(ids) {
  if (!Array.isArray(ids) || ids.length > nodeLimit) return false;
  const unique = new Set();
  for (const id of ids) {
    if (typeof id !== 'string' || !nodeIdPattern.test(id) || unique.has(id)) return false;
    unique.add(id);
  }
  return true;
}

function finiteRect(values) {
  return Array.isArray(values) && values.length === 4
    && values.every(value => Number.isFinite(value) && Math.abs(value) <= coordinateLimit)
    && values[2] > 0 && values[3] > 0;
}

function readViewBox(root) {
  const values = (root.getAttribute('viewBox') || '').trim().split(/[\s,]+/).map(Number);
  if (!finiteRect(values)) throw Error('renderer returned an invalid view box');
  return values;
}

function nodeRect(root, element) {
  const inverse = root.getScreenCTM()?.inverse();
  const bounds = element.getBoundingClientRect();
  if (!inverse || bounds.width <= 0 || bounds.height <= 0) {
    throw Error('renderer returned an unmeasurable node');
  }
  const points = [
    new DOMPoint(bounds.left, bounds.top),
    new DOMPoint(bounds.right, bounds.top),
    new DOMPoint(bounds.left, bounds.bottom),
    new DOMPoint(bounds.right, bounds.bottom),
  ].map(point => point.matrixTransform(inverse));
  const xs = points.map(point => point.x);
  const ys = points.map(point => point.y);
  const rect = [
    Math.min(...xs),
    Math.min(...ys),
    Math.max(...xs) - Math.min(...xs),
    Math.max(...ys) - Math.min(...ys),
  ];
  if (!finiteRect(rect)) throw Error('renderer returned invalid node geometry');
  return rect;
}

function readGeometry(root, nodeIds) {
  const expected = new Map(nodeIds.map(id => [id, {
    element: null,
    prefix: `${root.id}-flowchart-${id}-`,
  }]));
  for (const element of root.querySelectorAll('[data-id], .node[id]')) {
    const authored = element.getAttribute('data-id');
    for (const [id, entry] of expected) {
      const generated = element.id.startsWith(entry.prefix)
        && /^\d+$/.test(element.id.slice(entry.prefix.length));
      if (authored !== id && !generated) continue;
      if (entry.element && entry.element !== element) {
        throw Error('renderer returned duplicate node geometry');
      }
      entry.element = element;
    }
  }
  const nodes = nodeIds.map(id => {
    const { element } = expected.get(id);
    if (!element) throw Error('renderer omitted requested node geometry');
    return { id, rect: nodeRect(root, element) };
  });
  return { viewBox: readViewBox(root), nodes };
}

function isRequest(data) {
  return data && data.type === 'render' && data.protocol === protocol
    && data.generation === generation
    && typeof data.requestId === 'string' && /^[a-z0-9-]{1,64}$/.test(data.requestId)
    && typeof data.channel === 'string' && /^[a-z0-9-]{1,64}$/.test(data.channel)
    && Number.isSafeInteger(data.revision) && data.revision >= 0
    && (data.theme === 'light' || data.theme === 'dark')
    && typeof data.source === 'string' && data.source.length <= sourceLimit
    && validNodeIds(data.nodeIds);
}

addEventListener('message', async event => {
  if (event.source !== parent || event.origin !== parentOrigin || !isRequest(event.data)) return;
  const { requestId, channel, revision, theme, source, nodeIds } = event.data;
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
    const parsedRoot = inert.querySelector('svg');
    if (!parsedRoot || parsedRoot.namespaceURI !== 'http://www.w3.org/2000/svg') {
      throw Error('renderer did not return SVG');
    }
    const root = document.importNode(parsedRoot, true);
    renderHost.replaceChildren(root);
    const geometry = readGeometry(root, nodeIds);
    const svg = new XMLSerializer().serializeToString(root);
    if (encoder.encode(svg).length > resultLimit) throw Error('SVG exceeds output bound');
    reply({ type: 'rendered', svg, geometry });
  } catch (error) {
    reply({ type: 'error', error: String(error).slice(0, 500) });
  } finally {
    renderHost.replaceChildren();
    busy = false;
  }
});
parent.postMessage({ type: 'ready', protocol, generation }, parentOrigin);
