import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  rectSortingStrategy,
  sortableKeyboardCoordinates,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";

import { api } from "./api";
import {
  assetDisplayName,
  fileBasename,
  formatClock,
  formatSeconds,
  type EngineStatus,
  type ExportSummary,
  type PlannedItem,
  type ProgressEvent,
  type Project,
  type RecentProject,
  type Snapshot,
} from "./types";

// ---------------------------------------------------------------- helpers

interface Toast {
  message: string;
  tone: "info" | "error";
}

function parentDir(path: string): string {
  const trimmed = path.replace(/[/\\]+$/, "");
  const cut = trimmed.lastIndexOf("/");
  const cutWin = trimmed.lastIndexOf("\\");
  const idx = Math.max(cut, cutWin);
  return idx > 0 ? trimmed.slice(0, idx) : trimmed;
}

function projectNameFromDir(dir: string): string {
  return fileBasename(dir).replace(/\.lantern$/, "");
}

// ---------------------------------------------------------------- app

export default function App() {
  const [booting, setBooting] = useState(true);
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  const [engineError, setEngineError] = useState<string | null>(null);
  const [toast, setToast] = useState<Toast | null>(null);

  const showToast = useCallback(
    (message: string, tone: Toast["tone"] = "info") =>
      setToast({ message, tone }),
    [],
  );

  useEffect(() => {
    api
      .engineStatus()
      .then(setEngine)
      .catch((e) => setEngineError(String(e)));
    api
      .startupProject()
      .then((s) => setSnapshot(s))
      .catch(() => setSnapshot(null))
      .finally(() => setBooting(false));
  }, []);

  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(null), 8000);
    return () => clearTimeout(timer);
  }, [toast]);

  if (booting) {
    return (
      <div className="boot">
        <div className="boot-mark">✦</div>
        <p>Opening your slideshow…</p>
      </div>
    );
  }

  return (
    <>
      {snapshot ? (
        <Editor
          snapshot={snapshot}
          setSnapshot={setSnapshot}
          engineError={engineError}
          showToast={showToast}
          onSwitch={() => {
            api
              .projectClose()
              .catch(() => undefined)
              .finally(() => setSnapshot(null));
          }}
        />
      ) : (
        <Launch
          engine={engine}
          engineError={engineError}
          showToast={showToast}
          onOpened={setSnapshot}
        />
      )}
      {toast && (
        <div className={`toast ${toast.tone}`} role="status">
          {toast.message}
        </div>
      )}
    </>
  );
}

// ---------------------------------------------------------------- launch

function Launch({
  engine,
  engineError,
  showToast,
  onOpened,
}: {
  engine: EngineStatus | null;
  engineError: string | null;
  showToast: (message: string, tone?: Toast["tone"]) => void;
  onOpened: (snapshot: Snapshot) => void;
}) {
  const [recents, setRecents] = useState<RecentProject[]>([]);
  const [creating, setCreating] = useState<{
    folder: string | null;
    name: string;
  } | null>(null);

  const refreshRecents = useCallback(() => {
    api
      .recentProjects()
      .then(setRecents)
      .catch(() => setRecents([]));
  }, []);

  useEffect(refreshRecents, [refreshRecents]);

  const openExisting = async () => {
    const dir = await open({ directory: true, title: "Choose a slideshow folder" });
    if (!dir) return;
    try {
      onOpened(await api.projectOpen(dir));
    } catch (e) {
      showToast(String(e), "error");
    }
  };

  const startNew = async () => {
    const folder = await open({ directory: true, title: "Where should it live?" });
    if (!folder) return;
    setCreating({ folder, name: "My slideshow" });
  };

  const createProject = async () => {
    if (!creating?.folder) return;
    try {
      onOpened(await api.projectNew(creating.folder, creating.name));
    } catch (e) {
      showToast(String(e), "error");
    }
  };

  return (
    <div className="launch">
      <header className="launch-hero">
        <div className="hero-mark">✦</div>
        <h1>Lantern</h1>
        <p className="hero-sub">
          Turn a pile of photos, videos, and a favorite song into a video worth
          sharing. No timelines to learn, no codecs to pick — arrange your
          memories and press <strong>Create video</strong>.
        </p>
      </header>

      <div className="launch-actions">
        <button className="big-primary" onClick={startNew}>
          Start a new slideshow
        </button>
        <button className="big-ghost" onClick={openExisting}>
          Open an existing slideshow
        </button>
      </div>

      {recents.length > 0 && (
        <section className="recents">
          <h2>Pick up where you left off</h2>
          <ul>
            {recents.map((recent) => (
              <li key={recent.path}>
                <button
                  className="recent-item"
                  onClick={async () => {
                    try {
                      onOpened(await api.projectOpen(recent.path));
                    } catch (e) {
                      showToast(String(e), "error");
                      refreshRecents();
                    }
                  }}
                >
                  <span className="recent-name">{recent.name}</span>
                  <span className="recent-path">{recent.path}</span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}

      <EngineLine engine={engine} engineError={engineError} />

      {creating && (
        <Modal title="Name your slideshow" onClose={() => setCreating(null)}>
          <p className="modal-hint">
            Saved as a folder you can back up or move:{' '}
            <code>{creating.folder}</code>
          </p>
          <input
            className="text-input"
            value={creating.name}
            autoFocus
            onChange={(e) =>
              setCreating({ ...creating, name: e.target.value })
            }
            onKeyDown={(e) => e.key === "Enter" && createProject()}
          />
          <div className="modal-actions">
            <button className="ghost" onClick={() => setCreating(null)}>
              Cancel
            </button>
            <button className="primary" onClick={createProject}>
              Create
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

function EngineLine({
  engine,
  engineError,
}: {
  engine: EngineStatus | null;
  engineError: string | null;
}) {
  if (engineError) {
    return (
      <p className="engine-warning">
        The video engine couldn't start — exporting won't work until ffmpeg is
        installed. Everything else still works. ({engineError})
      </p>
    );
  }
  if (!engine) return null;
  return (
    <p className="engine-line">
      Video engine ready · ffmpeg {engine.version}
    </p>
  );
}

// ---------------------------------------------------------------- editor

interface ExportJob {
  phase: "running" | "done" | "error" | "cancelled";
  label: string;
  percent: number;
  message?: string;
  summary?: ExportSummary;
}

function Editor({
  snapshot,
  setSnapshot,
  engineError,
  showToast,
  onSwitch,
}: {
  snapshot: Snapshot;
  setSnapshot: (snapshot: Snapshot) => void;
  engineError: string | null;
  showToast: (message: string, tone?: Toast["tone"]) => void;
  onSwitch: () => void;
}) {
  const { project, plan, dir } = snapshot;
  const [previewOpen, setPreviewOpen] = useState(false);
  const [optionsOpen, setOptionsOpen] = useState(false);
  const [viewerIndex, setViewerIndex] = useState<number | null>(null);
  const [exportOpen, setExportOpen] = useState(false);
  const [exportJob, setExportJob] = useState<ExportJob | null>(null);
  const exporting = exportJob?.phase === "running";
  const exportingRef = useRef(false);
  exportingRef.current = exporting;

  const undoStack = useRef<Project[]>([]);

  const apply = useCallback(
    async (fn: () => Promise<Snapshot>) => {
      undoStack.current.push(structuredClone(project));
      if (undoStack.current.length > 60) undoStack.current.shift();
      try {
        setSnapshot(await fn());
      } catch (e) {
        showToast(String(e), "error");
      }
    },
    [project, setSnapshot, showToast],
  );

  const undo = useCallback(async () => {
    const previous = undoStack.current.pop();
    if (!previous) return;
    try {
      setSnapshot(await api.projectRestore(previous));
      showToast("Undone");
    } catch (e) {
      showToast(String(e), "error");
    }
  }, [setSnapshot, showToast]);

  const doImport = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) return;
      undoStack.current.push(structuredClone(project));
      if (undoStack.current.length > 60) undoStack.current.shift();
      try {
        const result = await api.importMedia(paths);
        setSnapshot(result.snapshot);
        if (result.added > 0) {
          showToast(`Added ${result.added} file${result.added === 1 ? "" : "s"}`);
        }
        if (result.skipped.length > 0) {
          showToast(
            result.skipped
              .map((s) => `“${fileBasename(s.path)}” — ${s.reason}`)
              .join("\n"),
            "error",
          );
        }
      } catch (e) {
        showToast(String(e), "error");
      }
    },
    [project, setSnapshot, showToast],
  );

  // Native drag-and-drop import.
  useEffect(() => {
    const registration = getCurrentWebviewWindow().onDragDropEvent((event) => {
      if (event.payload.type === "drop") {
        void doImport(event.payload.paths);
      }
    });
    return () => {
      void registration.then((unlisten) => unlisten());
    };
  }, [doImport]);

  // Export events.
  useEffect(() => {
    const percentFor = (event: ProgressEvent): number => {
      switch (event.phase) {
        case "preparing":
          return Math.round(((event.index - 0.5) / event.total) * 80);
        case "stitching":
          return 86;
        case "music":
          return 93;
        case "finishing":
          return 96;
        default:
          return 0;
      }
    };
    const listeners = [
      listen<ProgressEvent>("export-progress", (event) => {
        if (!exportingRef.current) return;
        setExportJob((job) =>
          job && job.phase === "running"
            ? {
                ...job,
                label: event.payload.label,
                percent: percentFor(event.payload),
              }
            : job,
        );
      }),
      listen<ExportSummary>("export-done", (event) => {
        if (!exportingRef.current) return;
        setExportJob({
          phase: "done",
          label: "Your video is ready",
          percent: 100,
          summary: event.payload,
        });
      }),
      listen("export-cancelled", () => {
        if (!exportingRef.current) return;
        setExportJob({
          phase: "cancelled",
          label: "Video creation cancelled",
          percent: 0,
        });
      }),
      listen<{ message: string }>("export-error", (event) => {
        if (!exportingRef.current) return;
        setExportJob({
          phase: "error",
          label: "The video couldn't be created",
          percent: 0,
          message: event.payload.message,
        });
      }),
    ];
    return () => {
      listeners.forEach((p) => void p.then((unlisten) => unlisten()));
    };
  }, []);

  // Keyboard: undo, close overlays, preview toggle handled inside overlays.
  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") {
        event.preventDefault();
        void undo();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [undo]);

  const startExport = async () => {
    const name = projectNameFromDir(dir) || "slideshow";
    const outPath = `${parentDir(dir)}/${name}.mp4`;
    try {
      await api.exportStart(outPath);
      setExportOpen(false);
      setExportJob({
        phase: "running",
        label: "Starting…",
        percent: 2,
      });
    } catch (e) {
      showToast(String(e), "error");
    }
  };

  const photoCount = plan.items.filter((i) => i.kind === "image").length;
  const videoCount = plan.items.filter((i) => i.kind === "video").length;
  const songAsset = project.audio
    ? project.assets[project.audio.assetId]
    : null;

  return (
    <div className="editor">
      <header className="topbar">
        <div className="brand">
          <span className="brand-mark">✦</span>
          <span className="brand-name">{projectNameFromDir(dir)}</span>
        </div>
        <div className="topbar-actions">
          <button
            className="ghost small"
            onClick={() => void undo()}
            disabled={undoStack.current.length === 0}
          >
            Undo
          </button>
          <button className="ghost small" onClick={onSwitch}>
            Switch slideshow
          </button>
          <button className="ghost small" onClick={() => setOptionsOpen(true)}>
            More options
          </button>
          <button
            className="secondary"
            onClick={() => setPreviewOpen(true)}
            disabled={plan.items.length === 0}
          >
            Preview
          </button>
          <button
            className="primary"
            onClick={() => {
              setExportJob(null);
              setExportOpen(true);
            }}
            disabled={plan.items.length === 0 || exporting}
          >
            Create video
          </button>
        </div>
      </header>

      {engineError && (
        <div className="strip warning">{engineError}</div>
      )}

      <main className="editor-body">
        <section className="strip-area">
          <div className="strip-head">
            <h2>
              {photoCount > 0 && `${photoCount} photo${photoCount === 1 ? "" : "s"}`}
              {photoCount > 0 && videoCount > 0 && " · "}
              {videoCount > 0 && `${videoCount} video${videoCount === 1 ? "" : "s"}`}
              {plan.items.length > 0 && (
                <>
                  {" · "}
                  <span className="strip-total">{formatClock(plan.totalDuration)}</span>
                </>
              )}
            </h2>
            <AddButton onAdd={doImport} />
          </div>
          {plan.items.length === 0 ? (
            <EmptyDrop />
          ) : (
            <Filmstrip
              plan={plan}
              onMove={async (from, to) => apply(() => api.itemMove(from, to))}
              onRemove={async (itemId) => apply(() => api.itemRemove(itemId))}
              onDuration={async (itemId, seconds) =>
                apply(() => api.itemSetDuration(itemId, seconds))
              }
              onFit={async (itemId, fit) => apply(() => api.itemSetFit(itemId, fit))}
              onTrim={async (itemId, trimStart, duration) =>
                apply(() => api.itemSetTrim(itemId, trimStart, duration))
              }
              onView={(index) => setViewerIndex(index)}
            />
          )}
        </section>

        <aside className="rail">
          <MusicCard
            snapshot={snapshot}
            apply={apply}
            showToast={showToast}
            songName={songAsset ? assetDisplayName(songAsset) : null}
          />
          <DurationCard
            defaultDuration={project.settings.defaultImageDuration}
            photoCount={photoCount}
            apply={apply}
          />
          <div className="rail-hint">
            <p>Drag cards to reorder · double-click a card to zoom in.</p>
            <p>
              Tap a time to change one photo · <kbd>Ctrl</kbd>+<kbd>Z</kbd> to
              undo.
            </p>
          </div>
        </aside>
      </main>

      {previewOpen && (
        <Preview
          snapshot={snapshot}
          onClose={() => setPreviewOpen(false)}
        />
      )}

      {viewerIndex !== null && plan.items[viewerIndex] && (
        <MediaViewer
          items={plan.items}
          index={viewerIndex}
          onIndexChange={setViewerIndex}
          onClose={() => setViewerIndex(null)}
        />
      )}

      {optionsOpen && (
        <OptionsSheet
          snapshot={snapshot}
          apply={apply}
          onClose={() => setOptionsOpen(false)}
        />
      )}

      {exportOpen && !exportJob && (
        <Modal title="Create your video" onClose={() => setExportOpen(false)}>
          <p className="modal-hint">
            You'll get a plain <strong>MP4 video</strong> — plays on phones,
            TVs, and computers, and is easy to share.
          </p>
          <div className="export-target">
            <span className="export-file">
              {projectNameFromDir(dir)}.mp4
            </span>
            <span className="export-dir">{parentDir(dir)}</span>
          </div>
          <ul className="export-summary">
            <li>{plan.items.length} clips</li>
            <li>{formatClock(plan.totalDuration)} long</li>
            <li>
              {project.settings.height >= 2000
                ? "4K"
                : project.settings.height >= 1000
                  ? "Full HD"
                  : "HD"}{" "}
              ·{" "}
              {project.settings.crf <= 19
                ? "Maximum quality"
                : project.settings.crf <= 22
                  ? "High quality"
                  : "Good quality"}
            </li>
            <li>{project.audio ? "With your music" : "No music"}</li>
          </ul>
          <div className="modal-actions">
            <button className="ghost" onClick={() => setExportOpen(false)}>
              Not now
            </button>
            <button className="primary" onClick={() => void startExport()}>
              Create video
            </button>
          </div>
        </Modal>
      )}

      {exportJob && exportJob.phase === "running" && (
        <div className="export-running">
          <div className="export-card">
            <p className="export-label">{exportJob.label}</p>
            <div className="progress">
              <div
                className="progress-fill"
                style={{ width: `${Math.max(3, exportJob.percent)}%` }}
              />
            </div>
            <div className="export-running-actions">
              <span className="export-percent">{exportJob.percent}%</span>
              <button
                className="ghost"
                onClick={() => void api.exportCancel().catch((e) => showToast(String(e), "error"))}
              >
                Cancel
              </button>
            </div>
            <p className="export-note">
              Keep this window open — you can keep working on the slideshow
              while it renders.
            </p>
          </div>
        </div>
      )}

      {exportJob && exportJob.phase === "done" && exportJob.summary && (
        <Modal title="Your video is ready ✓" onClose={() => setExportJob(null)}>
          <p className="export-done-path">{exportJob.summary.outPath}</p>
          <p className="modal-hint">
            {formatClock(exportJob.summary.totalDuration)} long — easy to share
            by text, email, or a USB stick. Encoded with{" "}
            {exportJob.summary.encoder}.
          </p>
          <div className="modal-actions">
            <button
              className="ghost"
              onClick={() =>
                void openPath(parentDir(exportJob.summary!.outPath)).catch((e) =>
                  showToast(String(e), "error"),
                )
              }
            >
              Open folder
            </button>
            <button
              className="primary"
              onClick={() =>
                void openPath(exportJob.summary!.outPath).catch((e) =>
                  showToast(String(e), "error"),
                )
              }
            >
              Play now
            </button>
          </div>
        </Modal>
      )}

      {(exportJob?.phase === "error" || exportJob?.phase === "cancelled") && (
        <Modal
          title={exportJob.phase === "error" ? "The video couldn't be created" : "Cancelled"}
          onClose={() => setExportJob(null)}
        >
          {exportJob.message && <p className="modal-error">{exportJob.message}</p>}
          <div className="modal-actions">
            <button className="ghost" onClick={() => setExportJob(null)}>
              Close
            </button>
            <button
              className="primary"
              onClick={() => {
                setExportJob(null);
                setExportOpen(true);
              }}
            >
              Try again
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- pieces

function AddButton({ onAdd }: { onAdd: (paths: string[]) => void }) {
  const add = async () => {
    const picked = await open({
      multiple: true,
      title: "Choose photos and videos",
    });
    if (picked) onAdd(Array.isArray(picked) ? picked : [picked]);
  };
  return (
    <button className="secondary small" onClick={() => void add()}>
      + Add photos & videos
    </button>
  );
}

function EmptyDrop() {
  return (
    <div className="empty-drop">
      <div className="empty-mark">⊞</div>
      <p>
        <strong>Drag photos and videos here</strong>
      </p>
      <p className="empty-sub">
        iPhone photos, JPEGs, home videos — dump the whole pile in. The order
        you drop them becomes the order on screen.
      </p>
    </div>
  );
}

function Filmstrip({
  plan,
  onMove,
  onRemove,
  onDuration,
  onFit,
  onTrim,
  onView,
}: {
  plan: Snapshot["plan"];
  onMove: (from: number, to: number) => void;
  onRemove: (itemId: string) => void;
  onDuration: (itemId: string, seconds: number) => void;
  onFit: (itemId: string, fit: "contain" | "cover") => void;
  onTrim: (itemId: string, trimStart: number, duration: number) => void;
  onView: (index: number) => void;
}) {
  const ids = plan.items.map((item) => item.itemId);
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  const dragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const from = ids.indexOf(String(active.id));
    const to = ids.indexOf(String(over.id));
    if (from >= 0 && to >= 0) onMove(from, to);
  };

  return (
    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={dragEnd}>
      <SortableContext items={ids} strategy={rectSortingStrategy}>
        <div className="strip">
          {plan.items.map((item, index) => (
            <FilmCard
              key={item.itemId}
              item={item}
              index={index}
              onRemove={() => onRemove(item.itemId)}
              onDuration={(seconds) => onDuration(item.itemId, seconds)}
              onFit={() => onFit(item.itemId, item.fit === "cover" ? "contain" : "cover")}
              onTrim={(trimStart, duration) =>
                onTrim(item.itemId, trimStart, duration)
              }
              onView={() => onView(index)}
            />
          ))}
        </div>
      </SortableContext>
    </DndContext>
  );
}

function FilmCard({
  item,
  index,
  onRemove,
  onDuration,
  onFit,
  onTrim,
  onView,
}: {
  item: PlannedItem;
  index: number;
  onRemove: () => void;
  onDuration: (seconds: number) => void;
  onFit: () => void;
  onTrim: (trimStart: number, duration: number) => void;
  onView: () => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: item.itemId });
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [trimOpen, setTrimOpen] = useState(false);
  const [trimDraft, setTrimDraft] = useState({ start: "", length: "" });

  const commitTrim = () => {
    setTrimOpen(false);
    const start = Number.parseFloat(trimDraft.start);
    const length = Number.parseFloat(trimDraft.length);
    if (!Number.isFinite(start) || !Number.isFinite(length)) return;
    const nextStart = Math.max(0, start);
    const nextLength = Math.min(600, Math.max(0.1, length));
    if (
      Math.abs(nextStart - item.trimStart) > 0.01 ||
      Math.abs(nextLength - item.duration) > 0.01
    ) {
      onTrim(nextStart, nextLength);
    }
  };

  const commit = () => {
    setEditing(false);
    const value = Number.parseFloat(draft);
    if (Number.isFinite(value) && Math.abs(value - item.duration) > 0.01) {
      onDuration(Math.min(120, Math.max(0.5, value)));
    }
  };

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
    zIndex: isDragging ? 5 : undefined,
  };

  const thumbnail = item.thumbnailPath ? convertFileSrc(item.thumbnailPath) : null;

  return (
    <div
      ref={setNodeRef}
      style={style}
      className={`film-card ${isDragging ? "dragging" : ""}`}
      onDoubleClick={(e) => {
        e.stopPropagation();
        onView();
      }}
      {...attributes}
      {...listeners}
    >
      {thumbnail ? (
        <img src={thumbnail} alt="" draggable={false} />
      ) : (
        <div className="card-unknown">{item.kind}</div>
      )}
      <div className="card-index">{index + 1}</div>
      <button
        className="card-view"
        title="Zoom in"
        onPointerDown={(e) => e.stopPropagation()}
        onClick={(e) => {
          e.stopPropagation();
          onView();
        }}
      >
        ⤢
      </button>
      <button
        className="card-remove"
        title="Remove"
        onPointerDown={(e) => e.stopPropagation()}
        onClick={(e) => {
          e.stopPropagation();
          onRemove();
        }}
      >
        ×
      </button>
      {item.kind === "video" && <span className="kind-badge">▶ video</span>}
      <div className="card-foot">
        {item.kind === "image" ? (
          editing ? (
            <input
              className="duration-edit"
              type="number"
              min={0.5}
              max={120}
              step={0.5}
              autoFocus
              defaultValue={item.duration}
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onPointerDown={(e) => e.stopPropagation()}
              onKeyDown={(e) => {
                e.stopPropagation();
                if (e.key === "Enter") commit();
                if (e.key === "Escape") setEditing(false);
              }}
              onBlur={commit}
            />
          ) : (
            <button
              className="duration-pill"
              onPointerDown={(e) => e.stopPropagation()}
              onClick={(e) => {
                e.stopPropagation();
                setDraft(String(item.duration));
                setEditing(true);
              }}
            >
              {formatSeconds(item.duration)}s
            </button>
          )
        ) : trimOpen ? (
          <span className="trim-edit" onPointerDown={(e) => e.stopPropagation()}>
            <input
              className="duration-edit trim-input"
              type="number"
              min={0}
              step={0.5}
              title="Start at (seconds)"
              autoFocus
              value={trimDraft.start}
              onChange={(e) =>
                setTrimDraft((d) => ({ ...d, start: e.target.value }))
              }
              onPointerDown={(e) => e.stopPropagation()}
              onKeyDown={(e) => {
                e.stopPropagation();
                if (e.key === "Enter") commitTrim();
                if (e.key === "Escape") setTrimOpen(false);
              }}
              onBlur={commitTrim}
            />
            <input
              className="duration-edit trim-input"
              type="number"
              min={0.1}
              step={0.5}
              title="Play for (seconds)"
              value={trimDraft.length}
              onChange={(e) =>
                setTrimDraft((d) => ({ ...d, length: e.target.value }))
              }
              onPointerDown={(e) => e.stopPropagation()}
              onKeyDown={(e) => {
                e.stopPropagation();
                if (e.key === "Enter") commitTrim();
                if (e.key === "Escape") setTrimOpen(false);
              }}
              onBlur={commitTrim}
            />
          </span>
        ) : (
          <button
            className="duration-pill"
            title={
              item.trimStart > 0
                ? `Trimmed: starts at ${formatSeconds(item.trimStart)}s — tap to change`
                : "Tap to trim this clip"
            }
            onPointerDown={(e) => e.stopPropagation()}
            onClick={(e) => {
              e.stopPropagation();
              setTrimDraft({
                start: String(item.trimStart),
                length: String(item.duration),
              });
              setTrimOpen(true);
            }}
          >
            {item.trimStart > 0
              ? `${formatSeconds(item.trimStart)}→${formatSeconds(item.duration)}s`
              : `${formatSeconds(item.duration)}s`}
          </button>
        )}
        <button
          className="fit-toggle"
          title="Toggle how the frame fills the screen"
          onPointerDown={(e) => e.stopPropagation()}
          onClick={(e) => {
            e.stopPropagation();
            onFit();
          }}
        >
          {item.fit === "cover" ? "fill" : "inside"}
        </button>
      </div>
    </div>
  );
}

// Full-size zoom viewer for inspecting one item while arranging.
function MediaViewer({
  items,
  index,
  onIndexChange,
  onClose,
}: {
  items: Snapshot["plan"]["items"];
  index: number;
  onIndexChange: (index: number) => void;
  onClose: () => void;
}) {
  const item = items[index];
  const isImage = item.kind !== "video";
  const stageRef = useRef<HTMLDivElement | null>(null);
  const dragRef = useRef<{ px: number; py: number; x: number; y: number } | null>(null);
  const [view, setView] = useState({ scale: 1, x: 0, y: 0 });

  const reset = () => setView({ scale: 1, x: 0, y: 0 });

  useEffect(() => {
    reset();
    dragRef.current = null;
  }, [item.itemId]);

  const zoomAt = (factor: number, clientX?: number, clientY?: number) => {
    const stage = stageRef.current;
    setView(({ scale, x, y }) => {
      const next = Math.min(8, Math.max(1, scale * factor));
      if (next === scale) return { scale, x, y };
      if (next === 1) return { scale: 1, x: 0, y: 0 };
      if (!stage || clientX === undefined || clientY === undefined) {
        return { scale: next, x, y };
      }
      // Keep the point under the cursor fixed while scaling.
      const rect = stage.getBoundingClientRect();
      const vx = clientX - (rect.left + rect.width / 2);
      const vy = clientY - (rect.top + rect.height / 2);
      const k = next / scale;
      return { scale: next, x: vx - (vx - x) * k, y: vy - (vy - y) * k };
    });
  };

  // React onWheel is passive; wheel needs preventDefault to stop page scroll.
  const zoomRef = useRef(zoomAt);
  zoomRef.current = zoomAt;
  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      zoomRef.current(event.deltaY < 0 ? 1.2 : 1 / 1.2, event.clientX, event.clientY);
    };
    stage.addEventListener("wheel", onWheel, { passive: false });
    return () => stage.removeEventListener("wheel", onWheel);
  }, []);

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
      if (event.key === "ArrowLeft") onIndexChange(Math.max(0, index - 1));
      if (event.key === "ArrowRight") onIndexChange(Math.min(items.length - 1, index + 1));
      if (event.key === "+" || event.key === "=") zoomRef.current(1.25);
      if (event.key === "-") zoomRef.current(1 / 1.25);
      if (event.key === "0") reset();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [index, items.length, onClose, onIndexChange]);

  return (
    <div className="viewer" role="dialog" aria-modal="true" aria-label="Zoomed photo">
      <div className="viewer-stage" ref={stageRef}>
        {isImage ? (
          <img
            className={`viewer-media ${view.scale > 1 ? "zoomed" : ""}`}
            src={convertFileSrc(item.mediaPath)}
            alt=""
            draggable={false}
            style={{ transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})` }}
            onPointerDown={(e) => {
              if (view.scale <= 1) return;
              dragRef.current = { px: e.clientX, py: e.clientY, x: view.x, y: view.y };
              e.currentTarget.setPointerCapture(e.pointerId);
            }}
            onPointerMove={(e) => {
              const drag = dragRef.current;
              if (!drag) return;
              setView((v) => ({ ...v, x: drag.x + e.clientX - drag.px, y: drag.y + e.clientY - drag.py }));
            }}
            onPointerUp={() => {
              dragRef.current = null;
            }}
            onPointerCancel={() => {
              dragRef.current = null;
            }}
            onDoubleClick={(e) => {
              zoomAt(view.scale > 1 ? 1 / view.scale : 3, e.clientX, e.clientY);
            }}
          />
        ) : (
          <video className="viewer-media" src={convertFileSrc(item.mediaPath)} controls autoPlay playsInline />
        )}
      </div>
      <div className="viewer-bar">
        <button
          className="viewer-nav"
          title="Previous item"
          disabled={index === 0}
          onClick={() => onIndexChange(index - 1)}
        >
          ‹
        </button>
        <span className="viewer-count">
          {index + 1} / {items.length}
        </span>
        <button
          className="viewer-nav"
          title="Next item"
          disabled={index === items.length - 1}
          onClick={() => onIndexChange(index + 1)}
        >
          ›
        </button>
        {isImage && (
          <span className="viewer-zoom">
            <button title="Zoom out" onClick={() => zoomAt(1 / 1.25)}>
              −
            </button>
            <button title="Fit to window" onClick={reset}>
              100%
            </button>
            <button title="Zoom in" onClick={() => zoomAt(1.25)}>
              +
            </button>
          </span>
        )}
        <span className="viewer-hint">
          {isImage ? "scroll to zoom · drag to pan · double-click to toggle" : "video"} · ← → flip · Esc close
        </span>
        <button className="viewer-close" onClick={onClose}>
          Close
        </button>
      </div>
    </div>
  );
}

function MusicCard({
  snapshot,
  apply,
  showToast,
  songName,
}: {
  snapshot: Snapshot;
  apply: (fn: () => Promise<Snapshot>) => void;
  showToast: (message: string, tone?: Toast["tone"]) => void;
  songName: string | null;
}) {
  const audio = snapshot.project.audio;
  const pickSong = async () => {
    const picked = await open({
      multiple: false,
      title: "Choose a song",
    });
    if (!picked) return;
    try {
      apply(() => api.audioSet(picked));
    } catch (e) {
      showToast(String(e), "error");
    }
  };

  const presets = [
    { label: "Soft", value: 0.5 },
    { label: "Balanced", value: 0.8 },
    { label: "Loud", value: 1.2 },
  ];

  return (
    <div className="rail-card">
      <h3>Music</h3>
      {audio && songName ? (
        <>
          <p className="song-name">{songName}</p>
          <p className="rail-sub">Plays in the background, fades out at the end.</p>
          <div className="chip-row">
            {presets.map((preset) => (
              <button
                key={preset.label}
                className={`chip ${Math.abs(audio.volume - preset.value) < 0.01 ? "active" : ""}`}
                onClick={() => apply(() => api.audioSetVolume(preset.value))}
              >
                {preset.label}
              </button>
            ))}
          </div>
          <div className="chip-row">
            <button className="chip" onClick={() => void pickSong()}>
              Change song
            </button>
            <button
              className="chip quiet"
              onClick={() => apply(() => api.audioRemove())}
            >
              No music
            </button>
          </div>
        </>
      ) : (
        <>
          <p className="rail-sub">
            Choose a song — it plays over the whole slideshow and fades out at
            the end.
          </p>
          <button className="secondary small" onClick={() => void pickSong()}>
            + Add music
          </button>
        </>
      )}
    </div>
  );
}

function DurationCard({
  defaultDuration,
  photoCount,
  apply,
}: {
  defaultDuration: number;
  photoCount: number;
  apply: (fn: () => Promise<Snapshot>) => void;
}) {
  const quick = [2, 3, 5];
  return (
    <div className="rail-card">
      <h3>How long each photo stays</h3>
      <div className="chip-row">
        {quick.map((seconds) => (
          <button
            key={seconds}
            className={`chip ${Math.abs(defaultDuration - seconds) < 0.01 ? "active" : ""}`}
            onClick={() => apply(() => api.makeAllPhotos(seconds))}
          >
            {seconds}s
          </button>
        ))}
      </div>
      <p className="rail-sub">
        {photoCount > 0
          ? `Tapping one sets all ${photoCount} photo${photoCount === 1 ? "" : "s"}. Tap a card's time to set just that one.`
          : "Default is 3 seconds per photo."}
      </p>
    </div>
  );
}

// ---------------------------------------------------------------- options

function OptionsSheet({
  snapshot,
  apply,
  onClose,
}: {
  snapshot: Snapshot;
  apply: (fn: () => Promise<Snapshot>) => void;
  onClose: () => void;
}) {
  const settings = snapshot.project.settings;
  const resolutions = [
    { label: "480", short: 480 },
    { label: "720", short: 720 },
    { label: "1080", short: 1080 },
    { label: "2K", short: 1440 },
    { label: "4K", short: 2160 },
  ];
  const shapes = [
    { label: "Wide · TV & computer", value: "wide" as const },
    { label: "Vertical · phone", value: "vertical" as const },
    { label: "Square", value: "square" as const },
  ];
  const qualities = [
    { label: "Maximum quality", crf: 18 },
    { label: "High quality", crf: 21 },
    { label: "Good · smaller file", crf: 24 },
  ];
  const encoders = [
    { label: "Automatic (recommended)", value: "auto" as const },
    { label: "Processor", value: "cpu" as const },
    { label: "Graphics card", value: "gpu" as const },
  ];
  const frameRates = [
    { label: "Film-like · 24", fps: 24 },
    { label: "Standard · 30", fps: 30 },
    { label: "Extra smooth · 60", fps: 60 },
  ];
  const backgrounds = [
    { label: "Blurred photo", value: "blur" as const },
    { label: "Black", value: "black" as const },
    { label: "White", value: "white" as const },
  ];

  const currentShape =
    settings.width > settings.height
      ? "wide"
      : settings.height > settings.width
        ? "vertical"
        : "square";
  const currentShort =
    currentShape === "square"
      ? settings.width
      : Math.min(settings.width, settings.height);
  const pairFor = (shape: string, short: number): [number, number] => {
    const long = short === 480 ? 854 : (short * 16) / 9;
    if (shape === "square") return [short, short];
    if (shape === "vertical") return [short, long];
    return [long, short];
  };

  return (
    <Modal title="More options" onClose={onClose}>
      <div className="option-group">
        <h4>Resolution</h4>
        <div className="chip-row">
          {resolutions.map((resolution) => (
            <button
              key={resolution.label}
              className={`chip ${
                (currentShape === "square"
                  ? settings.width === resolution.short
                  : currentShort === resolution.short)
                  ? "active"
                  : ""
              }`}
              onClick={() =>
                apply(() => {
                  const [width, height] = pairFor(
                    currentShape,
                    resolution.short,
                  );
                  return api.settingsSet({ width, height });
                })
              }
            >
              {resolution.label}
            </button>
          ))}
        </div>
      </div>
      <div className="option-group">
        <h4>Shape</h4>
        <div className="chip-row">
          {shapes.map((shape) => (
            <button
              key={shape.value}
              className={`chip ${currentShape === shape.value ? "active" : ""}`}
              onClick={() =>
                apply(() => {
                  const [width, height] = pairFor(
                    shape.value,
                    Math.min(currentShort, 2160),
                  );
                  return api.settingsSet({ width, height });
                })
              }
            >
              {shape.label}
            </button>
          ))}
        </div>
      </div>
      <div className="option-group">
        <h4>Quality</h4>
        <div className="chip-row">
          {qualities.map((quality) => (
            <button
              key={quality.crf}
              className={`chip ${settings.crf === quality.crf ? "active" : ""}`}
              onClick={() => apply(() => api.settingsSet({ crf: quality.crf }))}
            >
              {quality.label}
            </button>
          ))}
        </div>
      </div>
      <div className="option-group">
        <h4>Smoothness</h4>
        <div className="chip-row">
          {frameRates.map((frameRate) => (
            <button
              key={frameRate.fps}
              className={`chip ${settings.fps === frameRate.fps ? "active" : ""}`}
              onClick={() =>
                apply(() => api.settingsSet({ fps: frameRate.fps }))
              }
            >
              {frameRate.label}
            </button>
          ))}
        </div>
      </div>
      <div className="option-group">
        <h4>Encoding</h4>
        <div className="chip-row">
          {encoders.map((encoder) => (
            <button
              key={encoder.value}
              className={`chip ${settings.encoder === encoder.value ? "active" : ""}`}
              onClick={() =>
                apply(() => api.settingsSet({ encoder: encoder.value }))
              }
            >
              {encoder.label}
            </button>
          ))}
        </div>
        <p className="option-hint">
          Automatic uses your graphics card when it works, and the processor
          otherwise. Processor encoding always plays everywhere.
        </p>
      </div>
      <div className="option-group">
        <h4>Empty space around photos</h4>
        <div className="chip-row">
          {backgrounds.map((background) => (
            <button
              key={background.value}
              className={`chip ${settings.background === background.value ? "active" : ""}`}
              onClick={() =>
                apply(() => api.settingsSet({ background: background.value }))
              }
            >
              {background.label}
            </button>
          ))}
        </div>
      </div>
      <p className="rail-sub">
        Reordering, times, and quality changes only re-render what changed —
        exports get faster after the first one.
      </p>
      <div className="modal-actions">
        <button className="primary" onClick={onClose}>
          Done
        </button>
      </div>
    </Modal>
  );
}

// ---------------------------------------------------------------- preview

function Preview({
  snapshot,
  onClose,
}: {
  snapshot: Snapshot;
  onClose: () => void;
}) {
  const items = snapshot.plan.items;
  const [index, setIndex] = useState(0);
  const [playing, setPlaying] = useState(true);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const atEnd = index >= items.length;
  const item = atEnd ? null : items[index];

  const advance = useCallback(() => {
    setIndex((current) => current + 1);
  }, []);

  // Image timing.
  useEffect(() => {
    if (!item || item.kind !== "image" || !playing) return;
    const timer = setTimeout(advance, item.duration * 1000);
    return () => clearTimeout(timer);
  }, [item, playing, advance]);

  // Music follows play state.
  useEffect(() => {
    const audio = audioRef.current;
    if (!audio) return;
    if (playing && !atEnd) void audio.play().catch(() => undefined);
    else audio.pause();
  }, [playing, atEnd, index]);

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
      if (event.key === " ") {
        event.preventDefault();
        setPlaying((p) => !p);
      }
      if (event.key === "ArrowRight") setIndex((i) => Math.min(i + 1, items.length));
      if (event.key === "ArrowLeft") setIndex((i) => Math.max(i - 1, 0));
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onClose, items.length]);

  const songAsset = snapshot.project.audio
    ? snapshot.project.assets[snapshot.project.audio.assetId]
    : null;

  return (
    <div className="preview">
      <div className="preview-stage">
        {item && item.kind === "image" && (
          <img key={item.itemId} src={convertFileSrc(item.mediaPath)} alt="" />
        )}
        {item && item.kind === "video" && (
          <video
            key={item.itemId}
            ref={videoRef}
            src={convertFileSrc(item.mediaPath)}
            autoPlay={playing}
            muted
            playsInline
            onLoadedMetadata={(e) => {
              const video = e.currentTarget;
              if (item.trimStart > 0) video.currentTime = item.trimStart;
            }}
            onTimeUpdate={(e) => {
              const video = e.currentTarget;
              if (video.currentTime >= item.trimStart + item.duration - 0.05) {
                advance();
              }
            }}
            onEnded={advance}
          />
        )}
        {atEnd && (
          <div className="preview-end">
            <p>That's the whole slideshow.</p>
            <button
              className="secondary"
              onClick={() => {
                setIndex(0);
                setPlaying(true);
              }}
            >
              Watch again
            </button>
          </div>
        )}
      </div>
      {songAsset && (
        <audio
          ref={(node) => {
            audioRef.current = node;
            if (node) node.volume = snapshot.project.audio?.volume ?? 0.8;
          }}
          src={convertFileSrc(`${snapshot.dir}/${songAsset.fileName}`)}
          loop
        />
      )}
      <div className="preview-bar">
        <span>
          {atEnd
            ? `${items.length} / ${items.length}`
            : `${index + 1} / ${items.length} · ${formatClock(snapshot.plan.totalDuration)}`}
        </span>
        <span className="preview-hint">
          <kbd>Space</kbd> pause · <kbd>←</kbd> <kbd>→</kbd> skip ·{" "}
          <kbd>Esc</kbd> close
        </span>
        <button className="ghost small" onClick={onClose}>
          Close
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- modal

function Modal({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
}) {
  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onClose]);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h3>{title}</h3>
          <button className="modal-close" onClick={onClose}>
            ×
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}
