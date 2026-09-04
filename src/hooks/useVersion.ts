import { logger } from "../lib/logger";
import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

export function useVersion(defaultVersion = "0.1.0") {
  const [version, setVersion] = useState(defaultVersion);

  useEffect(() => {
    invoke<string>("get_app_version")
      .then(setVersion)
      .catch((e: unknown) => {
        if (import.meta.env.DEV) {
          logger.warn("useVersion", 
            "[useVersion] failed to read version:",
            e instanceof Error ? e.message : String(e),
          );
        }
        setVersion(defaultVersion);
      });
  }, [defaultVersion]);

  return version;
}
