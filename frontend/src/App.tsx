import { Routes, Route, useLocation } from "react-router-dom";
import { useEffect } from "react";
import ChatPage from "./pages/ChatPage";
import AuthScreen from "./pages/AuthScreen";
import NotFound from "./pages/NotFound";
import SettingsPage from "./pages/SettingsPage";

export default function App() {
  const { pathname } = useLocation();

  useEffect(() => {
    window.scrollTo({ top: 0, behavior: "instant" });
  }, [pathname]);

  /*
   * Every page in Mixer chat is full-bleed — there's no shared chrome.
   * Each component owns its own background video, top-left wordmark and
   * (when relevant) sidebar trigger. React Router only picks the page.
   *
   *   /                Home          (landing — Chillax title + composer)
   *   /chat/<id>       ChatPage      (real conversation view, dud transcript)
   *   /auth/screen     AuthScreen    (login / register)
   *   anything else    NotFound
   */
  return (
    <Routes>
      <Route path="/"              element={<ChatPage />} />
      <Route path="/chat/:id"      element={<ChatPage />} />
      <Route path="/settings"      element={<SettingsPage />} />
      <Route path="/auth/screen"   element={<AuthScreen />} />
      <Route path="*"              element={<NotFound />} />
    </Routes>
  );
}
