// Undo/redo stack. Covers task CRUD, schedule operations, habit CRUD.
// Sync operations are NOT included.
// Max history is configurable via setMaxHistory() (default 50).
// onUndo/onRedo callbacks fire with the action description so callers
// can show a toast/snackbar feedback to the user.

type UndoableAction = {
  description: string;
  undo: () => Promise<void>;
  redo: () => Promise<void>;
};

export const DEFAULT_MAX_HISTORY = 50;

class UndoRedoManager {
  private undoStack: UndoableAction[] = [];
  private redoStack: UndoableAction[] = [];
  private listeners: Set<() => void> = new Set();
  private maxHistory = DEFAULT_MAX_HISTORY;
  private undoCallback: ((description: string) => void) | null = null;
  private redoCallback: ((description: string) => void) | null = null;

  private notify() {
    this.listeners.forEach((l) => l());
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  canUndo(): boolean {
    return this.undoStack.length > 0;
  }

  canRedo(): boolean {
    return this.redoStack.length > 0;
  }

  getMaxHistory(): number {
    return this.maxHistory;
  }

  setMaxHistory(n: number) {
    if (!Number.isFinite(n) || n <= 0) return;
    this.maxHistory = Math.floor(n);
    // Trim existing stacks to the new limit
    while (this.undoStack.length > this.maxHistory) {
      this.undoStack.shift();
    }
    while (this.redoStack.length > this.maxHistory) {
      this.redoStack.shift();
    }
    this.notify();
  }

  setOnUndo(cb: ((description: string) => void) | null) {
    this.undoCallback = cb;
  }

  setOnRedo(cb: ((description: string) => void) | null) {
    this.redoCallback = cb;
  }

  push(action: UndoableAction) {
    this.undoStack.push(action);
    if (this.undoStack.length > this.maxHistory) {
      this.undoStack.shift();
    }
    this.redoStack = [];
    this.notify();
  }

  async undo(opts?: { silent?: boolean }): Promise<void> {
    const action = this.undoStack.pop();
    if (!action) return;
    // Push to redoStack before executing so a throw doesn't lose the action.
    this.redoStack.push(action);
    try {
      await action.undo();
    } catch (e) {
      // Execution failed — move the action back to undoStack so it can be retried.
      this.redoStack.pop();
      this.undoStack.push(action);
      throw e;
    }
    if (!opts?.silent) {
      this.undoCallback?.(action.description);
    }
    this.notify();
  }

  async redo(opts?: { silent?: boolean }): Promise<void> {
    const action = this.redoStack.pop();
    if (!action) return;
    // Push to undoStack before executing so a throw doesn't lose the action.
    this.undoStack.push(action);
    try {
      await action.redo();
    } catch (e) {
      // Execution failed — move the action back to redoStack so it can be retried.
      this.undoStack.pop();
      this.redoStack.push(action);
      throw e;
    }
    if (!opts?.silent) {
      this.redoCallback?.(action.description);
    }
    this.notify();
  }

  clear() {
    this.undoStack = [];
    this.redoStack = [];
    this.notify();
  }
}

export const undoRedo = new UndoRedoManager();
