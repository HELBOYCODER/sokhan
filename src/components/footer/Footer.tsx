import React, { useState, useEffect } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { useTranslation } from "react-i18next";
import { Cloud } from "lucide-react";

const Footer: React.FC = () => {
  const { t } = useTranslation();
  const [version, setVersion] = useState("");

  useEffect(() => {
    const fetchVersion = async () => {
      try {
        const appVersion = await getVersion();
        setVersion(appVersion);
      } catch (error) {
        console.error("Failed to get app version:", error);
        setVersion("1.0.0");
      }
    };

    fetchVersion();
  }, []);

  return (
    <div className="w-full border-t border-mid-gray/20 pt-3">
      <div className="flex justify-between items-center text-xs px-4 pb-3 text-text/60">
        <div className="flex items-center gap-2">
          <Cloud className="w-3.5 h-3.5 text-accent" />
          {/* eslint-disable-next-line i18next/no-literal-string */}
          <span>{t("footer.engine", "Google Online")}</span>
        </div>

        {/* eslint-disable-next-line i18next/no-literal-string */}
        <span>v{version}</span>
      </div>
    </div>
  );
};

export default Footer;
