import Markdown from "react-markdown";
import rehypeRaw from "rehype-raw";
import rehypeSanitize from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import { call } from "../api";
import { runAction } from "../store";

const imageHosts = new Set(["cdn.modrinth.com", "media.forgecdn.net", "mediafilez.forgecdn.net"]);
const projectHosts = new Set(["modrinth.com", "www.modrinth.com", "curseforge.com", "www.curseforge.com"]);

function allowedUrl(value: string | undefined, hosts: Set<string>, base?: string) {
  if (!value) return null;
  try {
    const url = new URL(value, base);
    return url.protocol === "https:" && !url.username && !url.password && !url.port && hosts.has(url.hostname) ? url.href : null;
  } catch {
    return null;
  }
}

export default function ProjectDescription({ body, projectUrl }: { body: string; projectUrl?: string | null }) {
  return <div className="project-prose">
    <Markdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeRaw, rehypeSanitize]} components={{
      a: ({ href, children }) => {
        const url = allowedUrl(href, projectHosts, projectUrl ?? undefined);
        return url ? <a href={url} onClick={(event) => {
          event.preventDefault();
          void runAction(() => call("open_project_url", { url }));
        }}>{children}</a> : <>{children}</>;
      },
      img: ({ src, alt }) => {
        const url = allowedUrl(src, imageHosts);
        return url ? <img src={url} alt={alt ?? ""} loading="lazy" referrerPolicy="no-referrer" /> : null;
      },
    }}>{body}</Markdown>
  </div>;
}
