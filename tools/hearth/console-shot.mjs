// tools/hearth/console-shot.mjs OUT_DIR - screenshots of the admin console of
// a throwaway instance (tools/hearth/throwaway.sh), signed in as its
// administrator, at desktop and phone width: the dashboard and the settings
// menu, to see that calendar, contact, file and WebDAV pages are gone.
//
// Needs Playwright; PLAYWRIGHT_FROM names a package.json beside an install
// (ES modules don't read NODE_PATH), e.g.
//   PLAYWRIGHT_FROM=/srv/hearth/web/package.json node tools/hearth/console-shot.mjs /tmp/out
import { createRequire } from 'node:module';
import { readFileSync, writeFileSync } from 'node:fs';
const { chromium } = createRequire(process.env.PLAYWRIGHT_FROM ?? import.meta.url)('playwright');

const out = process.argv[2] ?? '.';
const dir = process.env.HM_PROOF_DIR ?? '/srv/hearth-mail-proof';
const [user, secret] = readFileSync(`${dir}/admin`, 'utf8').split(/:(.*)/s);
const console_ = 'http://127.0.0.1:28081';
const notes = [];

const browser = await chromium.launch();
for (const [name, viewport] of [['desktop', { width: 1440, height: 900 }], ['phone', { width: 390, height: 844 }]]) {
  const page = await browser.newPage({ viewport, ignoreHTTPSErrors: true });
  await page.goto(console_, { waitUntil: 'networkidle' });
  await page.screenshot({ path: `${out}/console-${name}-start.png` });
  // The console sends people to the server's sign-in page and back
  const userField = page.locator('input[type=email], input[name=username], input[autocomplete=username]').first();
  if (await userField.count()) {
    await userField.fill(user);
    await page.locator('input[type=password]').first().fill(secret);
    await page.keyboard.press('Enter');
    await page.waitForLoadState('networkidle');
    await page.waitForTimeout(3000);
  }
  await page.screenshot({ path: `${out}/console-${name}-signed-in.png`, fullPage: true });
  const text = await page.locator('body').innerText();
  notes.push(`${name}: url=${page.url()} calendar=${/Calendar/i.test(text)} webdav=${/WebDAV/i.test(text)}`);
  // The settings menu, where Calendar & Contacts and WebDAV used to be
  const settings = page.getByText('Settings', { exact: true }).first();
  if (await settings.count()) {
    await settings.click().catch(() => {});
    await page.waitForTimeout(1500);
    await page.screenshot({ path: `${out}/console-${name}-settings.png`, fullPage: true });
    const menu = await page.locator('body').innerText();
    notes.push(`${name} settings: calendar=${/Calendar/i.test(menu)} webdav=${/WebDAV/i.test(menu)} contacts=${/Address Book|Contacts/i.test(menu)}`);
  }
  await page.close();
}
await browser.close();
writeFileSync(`${out}/console-notes.txt`, notes.join('\n') + '\n');
console.log(notes.join('\n'));
