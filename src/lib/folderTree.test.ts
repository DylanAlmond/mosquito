import { describe, expect, it } from 'vitest';

import fixtureJson from '../../test/fixtures/folder-tree/tree-input-and-outline.json';
import { buildFolderTree } from './folderTree';
import type { FolderNode } from './folderTree';
import type { SharedFile } from '../types';

// The canonical fixture both folder-tree mirrors' tests read (ADR-0002):
// this test and the Rust mirror's tests in
// crates/mosquito-core/src/folder_tree.rs must each derive the fixture's
// outline from its input list exactly. The former hand-maintained case
// lists are replaced by it — every rule they pinned is in the outline.
const FIXTURE = 'test/fixtures/folder-tree/tree-input-and-outline.json';

interface FixtureProvenance {
  folder_root: string;
  relative_path: string;
}

interface FixtureEntry {
  id: number;
  name: string;
  path: string;
  size: number;
  provenance: FixtureProvenance | null;
}

interface OutlineNode {
  name: string;
  folderRoot: string;
  subPath: string;
  depth: number;
  files: number[];
  children: OutlineNode[];
}

interface Fixture {
  notes: string[];
  input: FixtureEntry[];
  outline: { loose: number[]; roots: OutlineNode[] };
}

const fixture: Fixture = fixtureJson;

function sharedFilesFromFixture(): SharedFile[] {
  return fixture.input.map((entry) => ({
    id: entry.id,
    name: entry.name,
    path: entry.path,
    size: entry.size,
    provenance: entry.provenance
      ? { folder_root: entry.provenance.folder_root, relative_path: entry.provenance.relative_path }
      : null
  }));
}

function assertNodeMatches(node: FolderNode, expected: OutlineNode, spot: string): void {
  expect(node.name, `${FIXTURE}: ${spot} name disagrees`).toBe(expected.name);
  // Subfolder nodes inherit the root's absolute path.
  expect(node.folderRoot, `${FIXTURE}: ${spot} folderRoot disagrees`).toBe(expected.folderRoot);
  expect(node.subPath, `${FIXTURE}: ${spot} subPath disagrees`).toBe(expected.subPath);
  expect(node.depth, `${FIXTURE}: ${spot} depth disagrees`).toBe(expected.depth);
  expect(node.files.map((file) => file.id), `${FIXTURE}: ${spot} files (ids in shared-list order) disagree`).toEqual(
    expected.files
  );
  expect(
    node.children.map((child) => child.name),
    `${FIXTURE}: ${spot} children (first-appearance order) disagree`
  ).toEqual(expected.children.map((child) => child.name));
  expected.children.forEach((child, index) => {
    assertNodeMatches(node.children[index]!, child, `${spot}/${child.name}`);
  });
}

describe(`buildFolderTree — shared fixture (${FIXTURE})`, () => {
  it('derives the fixture input into exactly the fixture outline: loose files, root naming and order, files-before-subfolders, first-appearance ordering, arbitrary depth, duplicates, same-named roots and subfolders', () => {
    const tree = buildFolderTree(sharedFilesFromFixture());

    expect(tree.loose.map((file) => file.id), `${FIXTURE}: loose-file ids disagree`).toEqual(fixture.outline.loose);
    expect(
      tree.loose.every((file) => file.provenance === null),
      `${FIXTURE}: a listed loose file carried provenance`
    ).toBe(true);

    expect(
      tree.roots.map((root) => root.name),
      `${FIXTURE}: root names in first-appearance order disagree`
    ).toEqual(fixture.outline.roots.map((root) => root.name));
    tree.roots.forEach((root, index) => assertNodeMatches(root, fixture.outline.roots[index]!, `roots[${index}]`));
  });
});
