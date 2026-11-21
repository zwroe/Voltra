import { useEffect, useState } from "react";
import { Coffee, Download } from "lucide-react";
import { call, errorText } from "../api";
import { runAction, useLauncher } from "../store";
import { Button } from "./ui/button";

export function JavaRuntimes() {
  const go = useLauncher((state) => state.go);
  const [releases, setReleases] = useState<number[] | null>(null);
  const [major, setMajor] = useState(21);
  const [error, setError] = useState("");
  const [installing, setInstalling] = useState(false);
  useEffect(() => {
    let live = true;
    call<number[]>("available_java").then((data) => {
      if (live) setReleases([...new Set(data)].sort((a, b) => b - a));
    }).catch((reason) => {
      if (live) setError(errorText(reason));
    });
    return () => { live = false; };
  }, []);
  const choices = releases?.length ? releases : [25, 21, 17, 16, 11, 8];
  return <section className="java-runtimes">
    <div><Coffee size={17} /><strong>Download a Java runtime</strong></div>
    <div className="actions-wrap">
      <select aria-label="Java release" value={choices.includes(major) ? major : choices[0]}
        onChange={(event) => setMajor(Number(event.target.value))}>
        {choices.map((version) => <option value={version} key={version}>Java {version}</option>)}
      </select>
      <Button disabled={installing} onClick={() => {
        setInstalling(true);
        void runAction(async () => {
          await call("install_java", { major: choices.includes(major) ? major : choices[0] });
          go("Downloads");
          await useLauncher.getState().poll();
        }).finally(() => setInstalling(false));
      }}><Download /> Install Java</Button>
    </div>
    {error && <p className="field-hint">Showing common releases. Eclipse Adoptium’s catalog could not be refreshed.</p>}
  </section>;
}
