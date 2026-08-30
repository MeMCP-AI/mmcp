// Builds a folder-hierarchy tree from a flat list of memory
// descriptors by splitting each `slug` on '/'. Pure data transform:
// no store access, no Svelte state, so it is unit-testable in
// isolation and reusable by any list/tree view over the same
// descriptor shape.
//
// A slug with no '/' is a top-level leaf. Each '/'-separated
// prefix becomes (or reuses) a folder node; folders are keyed by
// their full prefix path so two entries sharing a prefix share one
// folder node instead of each spawning their own.
//
// Edge case: a slug that is simultaneously a leaf AND a folder
// prefix of other slugs (e.g. both `foo` and `foo/bar` exist).
// Chosen rule: `foo` renders as its own leaf node, sibling to the
// `foo/` folder node that `foo/bar` introduces, both at the same
// nesting depth. They are never merged into one node: a folder has
// no frontmatter of its own to show, so a leaf's row and a folder's
// row are different renderable shapes and collapsing them would
// either drop the leaf's row or invent frontmatter for the folder.

export interface MemoryTreeLeaf<T> {
  type: 'leaf';
  /** Full slug, unsplit, kept for keying and navigation. */
  slug: string;
  /** Last '/'-segment of the slug, the label shown for this row. */
  name: string;
  entry: T;
}

export interface MemoryTreeFolder<T> {
  type: 'folder';
  /** Single '/'-segment name, the label shown for this row. */
  name: string;
  /** Full '/'-joined prefix up to and including this folder, used as its stable identity. */
  path: string;
  children: MemoryTreeNode<T>[];
}

export type MemoryTreeNode<T> = MemoryTreeLeaf<T> | MemoryTreeFolder<T>;

/** Builds the tree in input order: within a folder, children appear
 * in the order their entries were first encountered. Callers that
 * want alphabetical or kind-grouped ordering sort `entries` first;
 * this function never reorders. */
export function buildMemoryTree<T extends { slug: string }>(
  entries: readonly T[]
): MemoryTreeNode<T>[] {
  const root: MemoryTreeNode<T>[] = [];
  const foldersByPath = new Map<string, MemoryTreeFolder<T>>();

  for (const entry of entries) {
    const segments = entry.slug.split('/');
    let siblings = root;
    let pathSoFar = '';
    for (let i = 0; i < segments.length - 1; i++) {
      pathSoFar = pathSoFar === '' ? segments[i] : `${pathSoFar}/${segments[i]}`;
      let folder = foldersByPath.get(pathSoFar);
      if (!folder) {
        folder = { type: 'folder', name: segments[i], path: pathSoFar, children: [] };
        foldersByPath.set(pathSoFar, folder);
        siblings.push(folder);
      }
      siblings = folder.children;
    }
    siblings.push({ type: 'leaf', slug: entry.slug, name: segments[segments.length - 1], entry });
  }

  return root;
}

/** Every folder `path` reachable in `nodes`, depth-first. Used to
 * force-expand a tree already pruned down to filter matches: every
 * folder that survives pruning holds at least one matching leaf, so
 * none of them should stay collapsed and hide that match. */
export function collectFolderPaths<T>(nodes: readonly MemoryTreeNode<T>[]): string[] {
  const paths: string[] = [];
  for (const node of nodes) {
    if (node.type === 'folder') {
      paths.push(node.path);
      paths.push(...collectFolderPaths(node.children));
    }
  }
  return paths;
}
