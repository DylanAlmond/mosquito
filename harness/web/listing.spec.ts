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

  // Visible rows: the two loose files, then the expanded Photographs
  // folder with its direct member. The nested member stays hidden
  // inside the collapsed `notes` subfolder.
  const rows = page.locator('.file-row:visible');
  await expect(rows).toHaveCount(3);

  const looseFirst = rows.nth(0);
  await expect(looseFirst).toContainText('recipes.txt');
  await expect(looseFirst).not.toHaveAttribute('data-depth');
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

test('renders the folder tree with roots expanded and subfolders collapsed', async ({ page }) => {
  await page.goto(FIXTURE_URL);

  // One folder row per tree node: the root at depth 0, the `notes`
  // subfolder deeper. The root is expanded, so its direct member
  // (beach.jpg) is visible right under it.
  const root = page.locator('.folder-row[data-folder-key="Photographs"]');
  await expect(root).toHaveCount(1);
  await expect(root).toHaveAttribute('data-depth', '0');
  await expect(root.locator('.folder-name')).toHaveText('Photographs');
  await expect(root.locator('[aria-expanded]')).toHaveAttribute('aria-expanded', 'true');
  await expect(root.locator('[aria-label="Collapse Photographs"]')).toHaveAttribute(
    'aria-expanded',
    'true'
  );

  await expect(
    page.locator('.file-row[data-depth="1"]:visible')
  ).toContainText('beach.jpg');

  // The subfolder starts collapsed: visible row, hidden member.
  const notes = page.locator('.folder-row[data-folder-key="Photographs/notes"]');
  await expect(notes).toBeVisible();
  await expect(notes).toHaveAttribute('data-depth', '1');
  await expect(notes.locator('[aria-label="Expand notes"]')).toHaveAttribute(
    'aria-expanded',
    'false'
  );
  const nestedMember = page.locator('.file-row[data-depth="2"]');
  await expect(nestedMember).toContainText('todo.txt');
  await expect(nestedMember).toBeHidden();

  // Depth alternates the shade and indents the name text 16px per
  // level (ADR-0004): the expanded
  // root's whole visible region carries the grey shade, and a
  // collapsed folder row keeps its parent's background.
  await expect(root).toHaveAttribute('data-shade', '1');
  await expect(page.locator('.file-row[data-depth="1"]:visible')).toHaveAttribute(
    'data-shade',
    '1'
  );
  await expect(notes).toHaveAttribute('data-shade', '1');
  // The indent itself: the name text grows 16px per depth and member
  // rows catch up the chevron slot (+36px), so everything under
  // `Photographs` sits past the folder's own name; loose rows and
  // folder roots stay flush.
  await expect(page.locator('.file-row[data-depth="1"]:visible a > span')).toHaveCSS(
    'padding-left',
    '52px'
  );
  await expect(
    page.locator('.folder-row[data-folder-key="Photographs/notes"] .folder-name')
  ).toHaveCSS('padding-left', '16px');
  await expect(page.locator('.folder-row[data-folder-key="Photographs"] .folder-name')).toHaveCSS(
    'padding-left',
    '0px'
  );
  await expect(page.locator('.file-row[data-depth="2"] a > span').first()).toHaveCSS(
    'padding-left',
    '68px'
  );
  await expect(page.locator('.file-row:not([data-depth]) a > span').first()).toHaveCSS(
    'padding-left',
    '0px'
  );

  await page.screenshot({ path: shot('folder-groups') });
});

test('toggles folders through their chevrons only', async ({ page }) => {
  await page.goto(FIXTURE_URL);

  // Clicking the row or its name must never toggle: only the chevron.
  const notes = page.locator('.folder-row[data-folder-key="Photographs/notes"]');
  await notes.click();
  await notes.locator('.folder-name').click();
  await expect(page.locator('.file-row[data-depth="2"]')).toBeHidden();

  await notes.locator('[aria-label="Expand notes"]').click();
  await expect(notes.locator('[aria-label="Collapse notes"]')).toHaveAttribute(
    'aria-expanded',
    'true'
  );
  // Its region now alternates away from the root's grey shade.
  await expect(page.locator('.file-row[data-depth="2"]:visible')).toContainText(
    'todo.txt'
  );
  await expect(page.locator('.file-row[data-depth="2"]:visible')).toHaveAttribute(
    'data-shade',
    '0'
  );
  await expect(notes).toHaveAttribute('data-shade', '0');
  await notes.locator('[aria-label="Collapse notes"]').click();
  await expect(page.locator('.file-row[data-depth="2"]')).toBeHidden();

  // Collapsing the root hides its whole span; the loose files stay.
  await page.locator('[aria-label="Collapse Photographs"]').click();
  await expect(page.locator('.folder-row[data-folder-key="Photographs"]')).toBeVisible();
  await expect(
    page.locator('.file-row[data-depth="1"]:visible')
  ).toHaveCount(0);

  // And it comes back in the same expanded shape.
  await page.locator('[aria-label="Expand Photographs"]').click();
  await expect(page.locator('.file-row[data-depth="1"]:visible')).toContainText(
    'beach.jpg'
  );

  await page.screenshot({ path: shot('folder-toggles') });
});

test('selects subtrees, counting members hidden inside collapsed subfolders', async ({ page }) => {
  await page.goto(FIXTURE_URL);

  // Select-all covers every file, the hidden nested member included.
  await page.locator('#select-all').click();
  await expect(page.locator('#select-all')).toHaveAttribute('aria-checked', 'true');
  await expect(page.locator('.file-row.is-selected')).toHaveCount(4);
  const downloadSelected = page.locator('#download-selected');
  await expect(downloadSelected).toHaveClass(/is-visible/);
  await expect(downloadSelected).toHaveAttribute('href', '/download/zip?ids=0,1,2,3');

  // Deselect everything again, then act on the Photographs subtree
  // while `notes` is still collapsed: both members go selected, one of
  // them invisible in the page (matches the screenshot, not just the DOM).
  await page.locator('#select-all').click();
  const photographsButton = page.locator('button[data-subtree="2,3"]');
  await photographsButton.click();
  await expect(photographsButton).toHaveAttribute('aria-checked', 'true');
  await expect(page.locator('.file-row.is-selected')).toHaveCount(2);
  await expect(downloadSelected).toHaveAttribute('href', '/download/zip?ids=2,3');

  // Deselecting the visible member (via its checkbox: row bodies are
  // download links on the web) keeps the hidden one selected, so the
  // folder shows a mixed subtree.
  const visibleMember = page.locator('.file-row[data-depth="1"]:visible');
  await visibleMember.locator('button[data-select="2"]').click();
  await expect(photographsButton).toHaveAttribute('aria-checked', 'mixed');
  await expect(downloadSelected).toHaveAttribute('href', '/download/3');

  // Batch download of the subtree therefore ships the collapsed
  // member: the selection equals exactly ids 2,3 in the zip link above
  // when both are picked.
  await photographsButton.click();
  await expect(downloadSelected).toHaveAttribute('href', '/download/zip?ids=2,3');

  // A subfolder acts only on its own subtree.
  await expect(page.locator('#select-all')).toHaveAttribute('aria-checked', 'mixed');
  const notesButton = page.locator('button[data-subtree="3"]');
  await notesButton.click();
  await expect(notesButton).toHaveAttribute('aria-checked', 'false');
  await expect(downloadSelected).toHaveAttribute('href', '/download/2');

  await page.screenshot({ path: shot('selections') });
});
