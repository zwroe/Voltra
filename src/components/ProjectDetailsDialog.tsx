import { lazy, Suspense, useEffect, useState } from "react";
import { ExternalLink, ImageOff, LoaderCircle } from "lucide-react";
import { call, errorText, platformNames, type GalleryImage, type Platform, type ProjectDetails } from "../api";
import { runAction } from "../store";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";

const ProjectDescription = lazy(() => import("./ProjectDescription"));

export type OpenProjectDetails = {
  id: string;
  platform: Platform;
  title: string;
  description?: string;
  url?: string | null;
  trigger: HTMLButtonElement;
};

function GalleryPhoto({ image, label }: { image: GalleryImage; label: string }) {
  const [src, setSrc] = useState<string | null>(image.url);
  return src ? <img className="project-gallery-photo" src={src} alt={label} referrerPolicy="no-referrer"
    onError={() => setSrc(src === image.thumbnailUrl ? null : image.thumbnailUrl)} />
    : <div className="project-gallery-unavailable"><ImageOff size={22} /><p>Image could not be loaded.</p></div>;
}

function ProjectGallery({ images }: { images: GalleryImage[] }) {
  const [active, setActive] = useState(0);
  if (!images.length) return <p className="project-empty muted">This project has no gallery images.</p>;
  const selected = Math.min(active, images.length - 1);
  const image = images[selected];
  const label = image.title || `Gallery image ${selected + 1}`;

  return <div className="project-gallery">
    <figure className="project-gallery-figure">
      <GalleryPhoto key={image.url} image={image} label={image.description || label} />
      {(image.title || image.description) && <figcaption>
        {image.title && <strong>{image.title}</strong>}{image.description && <p className="muted">{image.description}</p>}
      </figcaption>}
    </figure>
    {images.length > 1 && <div className="project-gallery-strip" role="group" aria-label="Gallery images">
      {images.map((item, index) => <button key={index} type="button" className="project-gallery-thumb"
        aria-label={item.title || `Gallery image ${index + 1}`} aria-pressed={index === selected} onClick={() => setActive(index)}>
        <img src={item.thumbnailUrl} alt="" loading="lazy" referrerPolicy="no-referrer" />
      </button>)}
    </div>}
  </div>;
}

export function ProjectDetailsDialog({ project, onClose }: { project: OpenProjectDetails; onClose: () => void }) {
  const [tab, setTab] = useState<"description" | "gallery">("description");
  const [details, setDetails] = useState<ProjectDetails | null>(null);
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const provider = platformNames[project.platform];

  useEffect(() => {
    let live = true;
    setError("");
    call<ProjectDetails>("project_details", { platform: project.platform, project: project.id })
      .then((value) => { if (live) setDetails(value); })
      .catch((cause) => { if (live) setError(errorText(cause)); });
    return () => { live = false; };
  }, [project.platform, project.id, attempt]);

  const url = project.url || details?.url;
  const summary = details?.summary || project.description;
  const body = details?.body?.trim();

  return <Dialog open title={project.title} description={provider} className="project-details-dialog"
    onOpenChange={(open) => { if (!open) onClose(); }}
    onCloseAutoFocus={(event) => { event.preventDefault(); project.trigger.focus(); }}>
    <div className="project-details-toolbar">
      <div className="tabs" aria-label="Project details">
        <button type="button" aria-pressed={tab === "description"} onClick={() => setTab("description")}>Description</button>
        <button type="button" aria-pressed={tab === "gallery"} onClick={() => setTab("gallery")}>
          Gallery{details && details.gallery.length > 0 ? ` (${details.gallery.length})` : ""}
        </button>
      </div>
      {url && <Button size="small" variant="ghost" onClick={() => void runAction(() => call("open_project_url", { url }))}>
        <ExternalLink size={14} /> Open on {provider}
      </Button>}
    </div>
    {error && <div className="project-load-error">
      <p className="inline-error" role="alert">{error}</p>
      <Button size="small" onClick={() => setAttempt((value) => value + 1)}>Retry</Button>
    </div>}
    {details?.descriptionUnavailable && tab === "description" && <div className="project-load-error">
      <p className="field-hint">The full description couldn't be loaded.</p>
      <Button size="small" onClick={() => setAttempt((value) => value + 1)}>Retry</Button>
    </div>}
    <div className="tab-content" key={tab}>
      {!details && !error && <p className="waiting" role="status"><LoaderCircle className="spin" size={17} /> Loading project details…</p>}
      {tab === "gallery" ? details && <ProjectGallery images={details.gallery} />
        : body ? <Suspense fallback={<p role="status" className="waiting">Opening description…</p>}>
          <ProjectDescription body={body} projectUrl={url} />
        </Suspense> : summary ? <p className="project-description-summary">{summary}</p>
          : details && <p className="project-empty muted">This project has no description.</p>}
    </div>
  </Dialog>;
}
