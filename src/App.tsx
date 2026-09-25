import { Routes, Route } from "react-router-dom";
import MainLayout from "./layouts/MainLayout";
import { TooltipProvider } from "./components/ui/tooltip";
import { NetIfaceProvider } from "./contexts/NetIfaceContext";
import Dashboard from "./pages/Dashboard";
import Cleanup from "./pages/Cleanup";
import Network from "./pages/Network";
import NetConfig from "./pages/NetConfig";
import Settings from "./pages/Settings";
import Services from "./pages/Services";
import About from "./pages/About";

import Environment from "./pages/Environment";
import Hardware from "./pages/Hardware";

export default function App() {
  return (
    <TooltipProvider>
      <NetIfaceProvider>
        <Routes>
          <Route element={<MainLayout />}>
            <Route path="/" element={<Dashboard />} />
            <Route path="/cleanup" element={<Cleanup />} />
            <Route path="/network" element={<Network />} />
            <Route path="/net-config" element={<NetConfig />} />
            <Route path="/settings" element={<Settings />} />
            <Route path="/services" element={<Services />} />
            <Route path="/environment" element={<Environment />} />
            <Route path="/hardware" element={<Hardware />} />
            <Route path="/about" element={<About />} />
          </Route>
        </Routes>
      </NetIfaceProvider>
    </TooltipProvider>
  );
}
