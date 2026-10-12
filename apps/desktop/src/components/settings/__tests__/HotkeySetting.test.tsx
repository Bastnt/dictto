import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { GeneralTab } from "../GeneralTab";
import { invoke } from "@tauri-apps/api/core";

describe("hotkey settings", () => {
  it("captures a shortcut and persists it", async () => {
    const onSave = vi.fn();
    render(<GeneralTab settings={{hotkey: "Ctrl+Win"}} onSave={onSave} onOpenLanguageModal={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", {name: "Change hotkey"}));
    const input = screen.getByLabelText("Record shortcut");
    fireEvent.keyDown(input, {code: "KeyD", key: "d", ctrlKey: true, shiftKey: true});
    fireEvent.keyUp(input, {code: "KeyD", key: "d", ctrlKey: true, shiftKey: true});
    expect(onSave).toHaveBeenCalledWith("hotkey", "Ctrl+Shift+D");
    expect(invoke).toHaveBeenCalledWith("set_hotkey_capture", {active: true});
    expect(invoke).toHaveBeenCalledWith("set_hotkey_capture", {active: false});
  });
  it("captures modifier-only shortcuts and rejects a bare letter", async () => {
    const onSave = vi.fn();
    render(<GeneralTab settings={{}} onSave={onSave} onOpenLanguageModal={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", {name: "Change hotkey"}));
    const input = screen.getByLabelText("Record shortcut");
    fireEvent.keyDown(input, {code: "KeyF", key: "f"});
    fireEvent.keyUp(input, {code: "KeyF", key: "f"});
    expect(onSave).not.toHaveBeenCalled();
    fireEvent.keyDown(input, {code: "MetaLeft", key: "Meta", ctrlKey: true, metaKey: true});
    fireEvent.keyUp(input, {code: "MetaLeft", key: "Meta", ctrlKey: true});
    expect(onSave).toHaveBeenCalledWith("hotkey", "Ctrl+Win");
  });
  it("cancels capture with Escape without saving", async () => {
    const onSave = vi.fn();
    render(<GeneralTab settings={{}} onSave={onSave} onOpenLanguageModal={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", {name: "Change hotkey"}));
    fireEvent.keyDown(screen.getByLabelText("Record shortcut"), {key: "Escape", code: "Escape"});
    expect(onSave).not.toHaveBeenCalled();
    expect(screen.queryByLabelText("Record shortcut")).toBeNull();
  });
});
