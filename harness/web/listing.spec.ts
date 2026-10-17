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
  await expect(page.locator('#select-all')).toHaveCount(0);
  await expect(page.locator('#download-selected')).toHaveCount(0);

  await page.screenshot({ path: shot('empty-list') });
});

test('shows loose files before any folder group', async ({ page }) => {
  await page.goto(FIXTURE_URL);

  const rows = page.locator('.file-row');
  // Fixture tree: `recipes.txt` and `checklist.txt` loose, then the
  // Photographs folder — same order in the DOM (ADR-0001 layout).
  await expect(rows).toHaveCount(4);

  const looseFirst = rows.nth(0);
  await expect(looseFirst).toContainText('recipes.txt');
  await expect(looseFirst).not.toHaveAttribute('data-group-index');

  const looseSecond = rows.nth(1);
  await expect(looseSecond).toContainText('checklist.txt');

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

test('groups folder members under their root header', async ({ page }) => {
  await page.goto(FIXTURE_URL);

  const groupHeaders = page.locator('.group-header');
  await expect(groupHeaders).toHaveCount(1);
  await expect(groupHeaders.locator('.group-name')).toHaveText('Photographs');

  // Both Photographs members hang off the header's render-order index.
  const groupMembers = page.locator('.file-row[data-group-index="0"]');
  await expect(groupMembers).toHaveCount(2);
  await expect(groupMembers.nth(0)).toContainText('beach.jpg');
  await expect(groupMembers.nth(1)).toContainText('notes/todo.txt');

  // Group and member checkboxes exist and start unchecked.
  await expect(page.locator('button[data-select-group="0"]')).toHaveAttribute(
    'aria-checked',
    'false'
  );
  // Every member checkbox exists and starts unchecked.
  await expect(
    page.locator('.file-row[data-group-index="0"] button[data-select][aria-checked="true"]')
  ).toHaveCount(0);

  await page.screenshot({ path: shot('folder-groups') });
});

test('drives selection through checkboxes', async ({ page }) => {
  await page.goto(FIXTURE_URL);

  // Select all: every row highlights and the download action appears,
  // pointing at the zip of the whole selection.
  await page.locator('#select-all').click();
  await expect(page.locator('#select-all')).toHaveAttribute(
    'aria-checked',
    'true'
  );
  await expect(page.locator('.file-row.is-selected')).toHaveCount(4);
  const downloadSelected = page.locator('#download-selected');
  await expect(downloadSelected).toHaveClass(/is-visible/);
  await expect(downloadSelected).toHaveAttribute(
    'href',
    '/download/zip?ids=0,1,2,3'
  );

  // Untoggle the group: members drop, the two loose files stay picked,
  // and the headers reflect the mixed/unchecked state.
  await page.locator('button[data-select-group="0"]').click();
  await expect(page.locator('button[data-select-group="0"]')).toHaveAttribute(
    'aria-checked',
    'false'
  );
  await expect(page.locator('#select-all')).toHaveAttribute(
    'aria-checked',
    'mixed'
  );
  await expect(page.locator('.file-row.is-selected')).toHaveCount(2);
  // The zip link now lists only the remaining selection.
  await expect(downloadSelected).toHaveAttribute('href', '/download/zip?ids=0,1');

  // One unchecked member makes the whole select-all mixed; with only the
  // clean files selected the selection stays loose-only.
  await expect(page.locator('.file-row.is-selected')).toHaveText([
    /recipes\.txt/,
    /checklist\.txt/,
  ]);

  await page.screenshot({ path: shot('selections') });
});
