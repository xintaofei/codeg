"use client"

import { PetPanel } from "./_components/PetPanel"

// Route loaded inside the desktop-only `pet-panel` window (opened by tapping
// the sprite). No params — it renders the live active-session list from the
// `pet://sessions` stream.
export default function PetPanelPage() {
  return <PetPanel />
}
