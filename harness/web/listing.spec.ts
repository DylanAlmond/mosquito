import { join } from 'node:path';
import { expect, test } from '@playwright/test';
import { EMPTY_URL, FIXTURE_URL, SCREENSHOT_DIR } from './ports';

/// Screenshot paths are deterministic: same name per state, every run,
/// landing gitignored under `test-artifacts/web-harness/`.
function shot(name: string) {
  return join(process.cwd(), SCREENSHOT_DIR, `${name}.png`);
}

test('reproduces the empty listing state', async ({ page }) => {
  await page.goto(EMPTY_URL);

  const empty = page.locator('.empty-state');
  await expect(empty).toHaveText('Nothing is being shared right now.');
  await expect(page.locator('.file-row')).toHaveCount(0);
  await expect(page.locator('.folder-row')).toHaveCount(0);
  await expect(page.locator('#select-all')).toHaveCount(0);
  await expect(page.locator('#download-selected')).toHaveCount(0);

  await page.screenshot({ path: shot('empty-list') });
});

test('shows loose files before any folder', async ({ page }) => {
  await page.goto(FIXTURE_URL);

  // The page renders the top level only: the two loose files, then one
  // collapsed row per root record. The Photographs root's fragment
  // (auto-fetched because the server stamped it expanded) adds its
  // direct member after the loose rows — but the loose rows stay first.
  const files = page.locator('.file-row');
  await expect(files).toHaveCount(3);

  const looseFirst = files.nth(0);
  await expect(looseFirst).toContainText('recipes.txt');
  await expect(looseFirst).not.toHaveAttribute('data-depth');
  const looseSecond = files.nth(1);
  await expect(looseSecond).toContainText('checklist.txt');
  await expect(looseSecond).not.toHaveAttribute('data-depth');

  const member = files.nth(2);
  await expect(member).toContainText('beach.jpg');
  await expect(member).toHaveAttribute('data-depth', '1');

  // Progressive enhancement: with JS running each checkbox is an
  // interactive button carrying its own aria-checked state (unchecked
  // at load).
  await expect(looseFirst.locator('button[data-select]')).toHaveAttribute(
    'aria-checked',
    'false'
  );
  await expect(page.locator('#select-all')).toHaveAttribute(
    'aria-checked',
    'false'
  );

  await page.screenshot({ path: shot('loose-files') });
});

test('loads the stamped root through its fragment and keeps nested members absent until expanded', async ({
  page,
}) => {
  await page.goto(FIXTURE_URL);

  // One folder row per tree node: the root at depth 0 — and nothing
  // beneath it until its fragment arrives.
  const root = page.locator('.folder-row[data-folder-key="Photographs"]');
  await expect(root).toHaveCount(1);
  await expect(root).toHaveAttribute('data-depth', '0');
  await expect(root).toHaveAttribute('data-node-root', '0');
  await expect(root).toHaveAttribute('data-node-sub', '');
  await expect(root.locator('.folder-name')).toHaveText('Photographs');
  await expect(root.locator('.folder-count')).toHaveText('2 files');
  await expect(root.locator('[aria-expanded]')).toHaveAttribute('aria-expanded', 'true');
  await expect(root.locator('[aria-label="Collapse Photographs"]')).toHaveAttribute(
    'aria-expanded',
    'true'
  );

  // The five-children stamp expanded the root, so its page auto-fetches:
  // the direct member (beach.jpg) renders at depth 1, and the notes
  // subfolder beside it.
  const member = page.locator('.file-row[data-depth="1"]');
  await expect(member).toContainText('beach.jpg');

  const notes = page.locator('.folder-row[data-folder-key="Photographs/notes"]');
  await expect(notes).toBeVisible();
  await expect(notes).toHaveAttribute('data-depth', '1');
  await expect(notes).toHaveAttribute('data-node-root', '0');
  await expect(notes).toHaveAttribute('data-node-sub', 'notes');
  await expect(notes.locator('[aria-label="Expand notes"]')).toHaveAttribute(
    'aria-expanded',
    'false'
  );

  // The nested member is ABSENT until its own node's page is fetched —
  // rows arrive only through listings, not as hidden markup.
  await expect(page.locator('.file-row[data-depth="2"]')).toHaveCount(0);

  // Depth alternates the shade and indents the name text 16px per
  // level (ADR-0004): the expanded root's region is grey, the notes
  // row keeps its parent's region, and the member sits inside it.
  await expect(root).toHaveAttribute('data-shade', '1');
  await expect(notes).toHaveAttribute('data-shade', '1');
  await expect(member).toHaveAttribute('data-shade', '1');

  // The indent itself: the name text grows 16px per depth and member
  // rows catch up the chevron slot (+36px), so everything under
  // `Photographs` sits past the folder's own name; loose rows and
  // folder roots stay flush.
  await expect(member.locator('a > span')).toHaveCSS('padding-left', '52px');
  await expect(notes.locator('.folder-name')).toHaveCSS('padding-left', '16px');
  await expect(root.locator('.folder-name')).toHaveCSS('padding-left', '0px');
  await expect(
    page.locator('.file-row:not([data-depth]) a > span').first()
  ).toHaveCSS('padding-left', '0px');

  await page.screenshot({ path: shot('folder-groups') });
});

test('toggles folders through their chevrons, pruning their rows on collapse', async ({
  page,
}) => {
  await page.goto(FIXTURE_URL);
  await expect(page.locator('.file-row[data-depth="1"]')).toContainText('beach.jpg');

  // Clicking the row or its name must never toggle: only the chevron.
  const notes = page.locator('.folder-row[data-folder-key="Photographs/notes"]');
  await notes.click();
  await notes.locator('.folder-name').click();
  await expect(page.locator('.file-row[data-depth="2"]')).toHaveCount(0);

  await notes.locator('[aria-label="Expand notes"]').click();
  await expect(notes.locator('[aria-label="Collapse notes"]')).toHaveAttribute(
    'aria-expanded',
    'true'
  );
  // Its region now alternates away from the root's grey shade, and its
  // member arrives through the sub-node's own fragment fetch.
  await expect(page.locator('.file-row[data-depth="2"]:visible')).toContainText(
    'todo.txt'
  );
  await expect(page.locator('.file-row[data-depth="2"]:visible')).toHaveAttribute(
    'data-shade',
    '0'
  );
  await expect(notes).toHaveAttribute('data-shade', '0');

  // Collapsing prunes the row out of the DOM (not just hidden).
  await notes.locator('[aria-label="Collapse notes"]').click();
  await expect(page.locator('.file-row[data-depth="2"]')).toHaveCount(0);

  // Collapsing the root prunes its whole span; the loose files stay.
  await page.locator('[aria-label="Collapse Photographs"]').click();
  await expect(page.locator('.folder-row[data-folder-key="Photographs"]')).toBeVisible();
  await expect(page.locator('.file-row[data-depth="1"]')).toHaveCount(0);
  await expect(page.locator('.folder-row[data-folder-key="Photographs/notes"]')).toHaveCount(0);

  // And it comes back in the same expanded shape: the re-fetch
  // re-materializes over the server's idempotent registrations.
  await page.locator('[aria-label="Expand Photographs"]').click();
  await expect(page.locator('.file-row[data-depth="1"]:visible')).toContainText(
    'beach.jpg'
  );

  await page.screenshot({ path: shot('folder-toggles') });
});

test('selection covers only the rendered rows and no folder carries a checkbox', async ({
  page,
}) => {
  const downloadSelected = page.locator('#download-selected');
  await page.goto(FIXTURE_URL);
  await expect(page.locator('.file-row[data-depth="1"]')).toContainText('beach.jpg');

  // No subtree payload anywhere (removed with the mirror, issue #25):
  // folder rows carry a spacer column, never a checkbox.
  await expect(page.locator('button[data-subtree]')).toHaveCount(0);
  await expect(
    page.locator('.folder-row button[role="checkbox"]')
  ).toHaveCount(0);
  await expect(
    page.locator('.folder-row[data-folder-key="Photographs"] .checkbox-spacer')
  ).toHaveCount(1);

  // Select-all covers exactly the ids rendered so far: 0–1 loose plus
  // 2 (beach.jpg); the lazily-known todo.txt is not even a row yet.
  await page.locator('#select-all').click();
  await expect(page.locator('#select-all')).toHaveAttribute('aria-checked', 'true');
  await expect(page.locator('.file-row.is-selected')).toHaveCount(3);
  await expect(downloadSelected).toHaveClass(/is-visible/);
  await expect(downloadSelected).toHaveAttribute('href', '/download/zip?ids=0,1,2');

  // Expanding `notes` materializes its member (id 3): select-all is
  // mixed until the new row joins the selection.
  await expect(page.locator('.file-row.is-selected')).toHaveCount(3);
  await page.locator('[aria-label="Expand notes"]').click();
  await expect(page.locator('button[data-select="3"]')).toBeVisible();
  await expect(page.locator('#select-all')).toHaveAttribute('aria-checked', 'mixed');

  // The next click finishes the job: all four rendered rows.
  await page.locator('#select-all').click();
  await expect(page.locator('#select-all')).toHaveAttribute('aria-checked', 'true');
  await expect(page.locator('.file-row.is-selected')).toHaveCount(4);
  await expect(downloadSelected).toHaveAttribute('href', '/download/zip?ids=0,1,2,3');

  // Partial selection through a single row's checkbox: the mixed state
  // shows, and a one-file selection links that file directly.
  await page.locator('button[data-select="3"]').click();
  await expect(page.locator('#select-all')).toHaveAttribute('aria-checked', 'mixed');
  await expect(downloadSelected).toHaveAttribute('href', '/download/zip?ids=0,1,2');

  await page.locator('button[data-select="1"]').click();
  await page.locator('button[data-select="0"]').click();
  await expect(downloadSelected).toHaveAttribute('href', '/download/2');

  await page.screenshot({ path: shot('selections') });
});
