import {createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
import {join} from 'node:path';
import {writeFile, readFile} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';

const require = createRequire(`${process.cwd()}/`);
const {chromium} = require('playwright');
const packageVersion = require('playwright/package.json').version;
if (packageVersion !== '1.62.1') throw new Error(`Unpinned Playwright: ${packageVersion}`);
const [publicDirectory, privateDirectory] = process.argv.slice(2);
if (!publicDirectory || !privateDirectory) throw new Error('Expected separate publication and private evidence directories');
const browser = await chromium.launch({headless:true});
const version = browser.version();
if (version !== '151.0.7922.34') throw new Error(`Unpinned Chromium: ${version}`);
const context = await browser.newContext({offline:true,viewport:{width:1280,height:960},deviceScaleFactor:1});
const denied = [];
const errors = [];
await context.route('**/*', async route => {
  const protocol = new URL(route.request().url()).protocol;
  if (protocol === 'file:' || protocol === 'data:') await route.continue();
  else { denied.push(route.request().url()); await route.abort(); }
});
const page = await context.newPage();
page.on('pageerror', error => errors.push(error.message));
page.on('request', request => { if (/^https?:/.test(request.url())) denied.push(request.url()); });

async function load(name) {
  await page.goto(pathToFileURL(join(publicDirectory,name)).href);
  await page.evaluate(async () => {
    await document.fonts.ready;
    await Promise.all([...document.images].map(image => image.complete ? Promise.resolve() : new Promise((resolve,reject) => { image.onload=resolve; image.onerror=reject; })));
  });
  const result = await page.evaluate(() => {
    const ids = [...document.querySelectorAll('[id]')].map(node => node.id);
    const duplicateIds = ids.filter((id,index) => ids.indexOf(id)!==index);
    const missingAnchors = [...document.querySelectorAll('a[href^="#"]')].map(node => node.getAttribute('href').slice(1)).filter(id => !document.getElementById(id));
    const missingImages = [...document.images].filter(image => !image.naturalWidth || !image.alt).length;
    const unlabeledTables = [...document.querySelectorAll('table')].filter(table => !table.caption || !table.querySelector('th[scope]')).length;
    return {duplicateIds,missingAnchors,missingImages,unlabeledTables,language:document.documentElement.lang};
  });
  if (result.duplicateIds.length || result.missingAnchors.length || result.missingImages || result.unlabeledTables || result.language!=='en') throw new Error(`Semantic admission ${name}: ${JSON.stringify(result)}`);
  return result;
}

async function reflow(width,zoom) {
  await page.setViewportSize({width,height:900});
  await page.evaluate(zoom => { document.documentElement.style.zoom=String(zoom); },zoom);
  const overflow = await page.evaluate(() => {
    const width = innerWidth;
    return [...document.querySelectorAll('main *')].filter(node => {
      if (node.closest('.table-wrap')) return false;
      const box=node.getBoundingClientRect();
      return box.width>0 && (box.left < -1 || box.right > width+1);
    }).map(node => node.tagName).slice(0,10);
  });
  if (overflow.length) throw new Error(`Reflow overflow at ${width}px / ${zoom}: ${overflow.join(',')}`);
  return {viewport:width,zoom,overflow:0};
}

try {
  const tables = await load('tables.html');
  await page.keyboard.press('Tab');
  const tableFocus = await page.evaluate(() => document.activeElement?.classList.contains('skip'));
  if (!tableFocus) throw new Error('Evidence keyboard skip link is not first focus');
  await page.keyboard.press('Enter');
  if (new URL(page.url()).hash !== '#evidence') throw new Error('Evidence skip navigation failed');
  const tableReflow = [await reflow(380,1),await reflow(1280,2)];
  await page.evaluate(() => {document.documentElement.style.zoom='1';});
  await page.setViewportSize({width:1280,height:960});
  const report = await load('index.html');
  await page.keyboard.press('Tab');
  if (!await page.evaluate(() => document.activeElement?.classList.contains('skip'))) throw new Error('Report keyboard skip link is not first focus');
  await page.keyboard.press('Enter');
  if (new URL(page.url()).hash !== '#report') throw new Error('Report skip navigation failed');
  const reportReflow = [await reflow(380,1),await reflow(1280,2)];
  await page.evaluate(() => {document.documentElement.style.zoom='1';});
  await page.setViewportSize({width:1280,height:960});
  await page.screenshot({path:join(privateDirectory,'report-screen.png'),fullPage:true});
  await page.emulateMedia({media:'print'});
  const geometry = await page.evaluate(() => [...document.querySelectorAll('.sheet')].map((sheet,index) => {
    const box=sheet.getBoundingClientRect();
    const footer=sheet.querySelector('.footer').getBoundingClientRect();
    const visible=[...sheet.querySelectorAll('.page-content *')].map(node=>node.getBoundingClientRect()).filter(rect=>rect.width>0 && rect.height>0);
    const bottom=Math.max(box.top,...visible.map(rect=>rect.bottom));
    const horizontal=visible.filter(rect=>rect.left<box.left+1 || rect.right>box.right-1).length;
    return {page:index+1,width_mm:box.width*25.4/96,height_mm:box.height*25.4/96,footer_clearance_mm:(footer.top-bottom)*25.4/96,horizontal_overflow:horizontal};
  }));
  if (!geometry.length || geometry.some(row=>row.footer_clearance_mm<2 || row.horizontal_overflow || Math.abs(row.width_mm-210)>0.1 || Math.abs(row.height_mm-297)>0.1)) throw new Error(`Print geometry refusal: ${JSON.stringify(geometry)}`);
  // file:// links resolve to local machine paths in PDF annotations. Keep evidence
  // labels in print; the separately supplied HTML owns offline hyperlink access.
  await page.evaluate(() => { for (const link of document.querySelectorAll('a[href]')) link.removeAttribute('href'); });
  const pdf=join(publicDirectory,'making-work-land.pdf');
  await page.pdf({path:pdf,preferCSSPageSize:true,printBackground:true,tagged:true,outline:true,displayHeaderFooter:false});
  const info=execFileSync('pdfinfo',[pdf],{encoding:'utf8'});
  const count=Number(info.match(/^Pages:\s+(\d+)/m)?.[1]);
  if (count!==geometry.length) throw new Error(`PDF page count ${count} differs from ${geometry.length} admitted sheets`);
  const fontInfo=execFileSync('pdffonts',[pdf],{encoding:'utf8'});
  const fontRows=fontInfo.trim().split('\n').slice(2);
  if (!fontRows.length || fontRows.some(row=>!/^.*\s+yes\s+(?:yes|no)\s+(?:yes|no)\s+\d+\s+\d+\s*$/.test(row))) throw new Error(`Unembedded PDF fonts: ${fontInfo}`);
  const bytes=await readFile(pdf);
  // Spelled in parts so this detector is not itself a local-path finding.
  if (bytes.includes(Buffer.from('file:///')) || bytes.includes(Buffer.from('/'+'Users/'))) throw new Error('PDF contains local machine link material');
  execFileSync('pdftotext',['-layout',pdf,join(privateDirectory,'report-text.txt')]);
  if (denied.length || errors.length) throw new Error(`Offline refusal: requests=${denied.length}, errors=${errors.length}`);
  const publicReceipt={engine:'Chromium',engine_version:version,playwright_version:packageVersion,network_disabled:true,network_requests:0,pages:count,page_size:'A4 portrait',font_embedding_checked:true,keyboard_skip_links_checked:true,reflow:[{viewport:380,zoom:1},{viewport:1280,zoom:2}],minimum_footer_clearance_mm:Math.min(...geometry.map(row=>row.footer_clearance_mm))};
  await writeFile(join(privateDirectory,'print-admission-private.json'),JSON.stringify({publicReceipt,geometry,report,tables,reportReflow,tableReflow,pdfinfo:info,pdffonts:fontInfo},null,2));
  console.log(JSON.stringify(publicReceipt));
} finally {
  await context.close();
  await browser.close();
}
