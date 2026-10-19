import * as netkey from "./netkey-src.mjs";
import QRCode from "qrcode";

const api = {
  ...netkey,
  qrDataUrl(blob) {
    return QRCode.toDataURL(blob, { margin: 1, width: 180, errorCorrectionLevel: "L" });
  },
};

if (typeof window !== "undefined") window.PJNetKey = api;
export default api;
