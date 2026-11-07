import { listen, Event } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';

interface FilePayload {
  path: string;
  name: string;
  previewDataUrl?: string;
}

interface AppState {
  currentFilePath: string | null;
  isProcessing: boolean;
}

const appWindow = getCurrentWindow();
const state: AppState = {
  currentFilePath: null,
  isProcessing: false,
};

function getElement<T extends HTMLElement>(id: string): T {
  const el = document.getElementById(id);
  if (!el) {
    throw new Error(`[UI Error] Missing required DOM element: #${id}`);
  }
  return el as T;
}

function setPreviewUnavailable(message = 'Preview unavailable for this file format'): void {
  const previewContainer = document.getElementById('preview-container');
  if (previewContainer) {
    previewContainer.innerHTML = `<span class="preview-empty">📄 ${message}</span>`;
  }
}

function renderPayload(payload: FilePayload): void {
  const fileNameEl = getElement('file-name');
  const previewContainer = getElement('preview-container');

  state.currentFilePath = payload.path;

  fileNameEl.innerText = payload.name;
  fileNameEl.setAttribute('title', payload.path);

  if (payload.previewDataUrl) {
    const img = document.createElement('img');
    img.alt = `Preview of ${payload.name}`;
    img.src = payload.previewDataUrl;
    img.loading = 'eager';
    img.decoding = 'sync';

    img.onerror = () => {
      console.warn('Failed to load base64 preview image; reverting to fallback.');
      setPreviewUnavailable('Unable to render image preview');
    };

    previewContainer.replaceChildren(img);
  } else {
    setPreviewUnavailable();
  }
}

async function dismissWindow(): Promise<void> {
  console.log('[Action] Dismiss clicked');
  if (state.isProcessing) return;
  try {
    await appWindow.hide();
  } catch (err) {
    console.error('Failed to hide window:', err);
  }
}

async function openCurrentFile(): Promise<void> {
  console.log('[Action] Open File clicked - target:', state.currentFilePath);
  if (state.isProcessing || !state.currentFilePath) return;

  try {
    state.isProcessing = true;
    // The backend only opens the file it announced, never a path supplied by the webview.
    await invoke('open_received_file');
    await appWindow.hide();
  } catch (err) {
    console.error(`Failed to open file via Rust:`, err);
    setPreviewUnavailable('Failed to open file in default application');
  } finally {
    state.isProcessing = false;
  }
}

// ==========================================
// Reliable UI & Event Binding
// ==========================================

function setupUI(): void {
  try {
    const closeBtn = document.getElementById('close-btn');
    const openBtn = document.getElementById('open-btn');

    closeBtn?.addEventListener('click', dismissWindow);
    openBtn?.addEventListener('click', openCurrentFile);

    console.log('[✔] Button listeners bound successfully.');
  } catch (err) {
    console.error('Failed to bind buttons:', err);
  }
}

// Bind immediately if DOM is parsed, otherwise wait for DOMContentLoaded
if (document.readyState === 'loading') {
  window.addEventListener('DOMContentLoaded', setupUI);
} else {
  setupUI();
}

// Listen for backend Rust file events
listen<FilePayload>('new-file-received', async (event: Event<FilePayload>) => {
  console.log('New file received:', event.payload);
  try {
    renderPayload(event.payload);
  } catch (domErr) {
    console.error('DOM rendering error:', domErr);
  }
});

// Escape = Dismiss. There is deliberately no Enter = Open shortcut: a keystroke typed
// as the alert appears must never open a file a peer just sent.
window.addEventListener('keydown', async (event: KeyboardEvent) => {
  if (event.key === 'Escape') {
    event.preventDefault();
    await dismissWindow();
  }
});