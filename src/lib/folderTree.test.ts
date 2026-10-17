import { describe, expect, it } from 'vitest';
import { buildFolderTree } from './folderTree';
import type { SharedFile } from '../types';

function file(id: number, name: string, provenance: SharedFile['provenance']): SharedFile {
  return { id, name, path: `C:/pics/${name}`, size: 7, provenance };
}

function member(id: number, relativePath: string, folderRoot: string): SharedFile {
  return {
    id,
    name: relativePath.split('/').pop()!,
    path: `${folderRoot}/${relativePath}`,
    size: 7,
    provenance: { folder_root: folderRoot, relative_path: relativePath }
  };
}

describe('buildFolderTree', () => {
  it('keeps loose files at the top and turns each folder root into a root node', () => {
    const photos = 'C:/Photos';
    const files = [
      member(1, 'beach.jpg', photos),
      file(2, 'notes.txt', null),
      member(3, 'a.txt', photos)
    ];

    const tree = buildFolderTree(files);

    expect(tree.loose.map((f) => f.id)).toEqual([2]);
    expect(tree.roots).toHaveLength(1);
    expect(tree.roots[0]!.name).toBe('Photos');
    expect(tree.roots[0]!.folderRoot).toBe(photos);
    expect(tree.roots[0]!.files.map((f) => f.id)).toEqual([1, 3]);
    expect(tree.roots[0]!.children).toEqual([]);
  });

  it('holds directly-contained files before the subfolder children', () => {
    const photos = 'C:/Photos';
    const files = [
      member(1, 'travel/beach.jpg', photos),
      member(2, 'sunrise.jpg', photos),
      member(3, 'notes/todo.txt', photos)
    ];

    const tree = buildFolderTree(files);

    const root = tree.roots[0]!;
    expect(root.files.map((f) => f.id)).toEqual([2]);
    expect(root.children.map((c) => c.name)).toEqual(['travel', 'notes']);
    expect(root.children[0]!.files.map((f) => f.id)).toEqual([1]);
  });

  it('unwraps arbitrarily deep nesting as one subfolder per segment, with depth set', () => {
    const photos = 'C:/Photos';
    const files = [member(1, 'travel/beach/sunset.jpg', photos)];

    const tree = buildFolderTree(files);

    const root = tree.roots[0]!;
    expect(root.depth).toBe(0);
    expect(root.files).toEqual([]);
    expect(root.children.map((c) => c.subPath)).toEqual(['travel']);
    expect(root.children[0]!.depth).toBe(1);
    expect(root.children[0]!.children.map((c) => c.subPath)).toEqual(['travel/beach']);
    expect(root.children[0]!.children[0]!.depth).toBe(2);
    expect(root.children[0]!.children[0]!.files.map((f) => f.id)).toEqual([1]);
  });

  it('lists subfolders in first-appearance (add) order, not alphabetical', () => {
    const photos = 'C:/Photos';
    const files = [
      member(1, 'zebra/file-a.txt', photos),
      member(2, 'alpha/file-b.txt', photos),
      member(3, 'zebra/file-c.txt', photos)
    ];

    const tree = buildFolderTree(files);

    expect(tree.roots[0]!.children.map((c) => c.name)).toEqual(['zebra', 'alpha']);
    expect(tree.roots[0]!.children[0]!.files.map((f) => f.id)).toEqual([1, 3]);
  });

  it('routes same-named files and subfolders to the same root nodes without merging by name alone', () => {
    const files = [
      member(1, 'a/beach.jpg', 'C:/one/Photos'),
      member(2, 'b/a/beach.jpg', 'D:/two/Photos'),
      member(3, 'beach.jpg', 'D:/two/Photos')
    ];

    const tree = buildFolderTree(files);

    expect(tree.roots.map((r) => r.name)).toEqual(['Photos', 'Photos']);
    expect(tree.roots[0]!.children.map((c) => c.name)).toEqual(['a']);
    expect(tree.roots[1]!.children.map((c) => c.name)).toEqual(['b']);
    expect(tree.roots[1]!.files.map((f) => f.id)).toEqual([3]);
  });

  it('keeps a file added twice as two entries at the same spot', () => {
    const photos = 'C:/Photos';
    const files = [member(1, 'beach.jpg', photos), member(2, 'beach.jpg', photos)];

    const tree = buildFolderTree(files);

    expect(tree.roots[0]!.files.map((f) => f.id)).toEqual([1, 2]);
    expect(tree.roots[0]!.children).toEqual([]);
  });
});
