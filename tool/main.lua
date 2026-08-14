-- Renoise2Mod: export the current song to .mod/.xm via the bundled native renoise2mod binary.

local function bundle_path()
  return renoise.tool().bundle_path
end

local function binary_path()
  local platform = os.platform()
  local dir = bundle_path() .. "bin/"

  if platform == "WINDOWS" then
    return dir .. "renoise2mod-windows.exe"
  elseif platform == "MACINTOSH" then
    return dir .. "renoise2mod-darwin"
  else
    return dir .. "renoise2mod-linux"
  end
end

local function quote(s)
  return '"' .. tostring(s):gsub('"', '\\"') .. '"'
end

local function default_output_path(xrns_path, format)
  local base = xrns_path:match("^(.*)%.xrns$") or xrns_path
  return base .. "." .. format
end

local function read_file(path)
  local f = io.open(path, "r")
  if not f then
    return nil
  end
  local content = f:read("*a")
  f:close()
  return content
end

-- os.execute's success return is not consistent across Lua versions: Lua 5.4 returns a boolean
-- (true on exit code 0), while Lua 5.1/LuaJIT returns the raw numeric exit code directly (0 on
-- success). Handle both so this doesn't misreport success as failure depending on which Lua
-- Renoise happens to embed.
local function execute_succeeded(result)
  if type(result) == "boolean" then
    return result
  elseif type(result) == "number" then
    return result == 0
  else
    return false
  end
end

-- Runs the bundled CLI and returns (success, output_path, log_text).
local function run_export(xrns_path, output_path, opts)
  local log_path = os.tmpname()

  local args = {
    quote(binary_path()),
    quote(xrns_path),
    "--type", opts.format,
    "--out", quote(output_path),
    "--volscal", opts.volscal,
    "--log", quote(log_path),
  }

  if opts.format == "mod" then
    table.insert(args, "--ptmode")
    table.insert(args, opts.ptmode)
    table.insert(args, "--portresh")
    table.insert(args, tostring(opts.portresh))
    table.insert(args, "--sample-rate")
    table.insert(args, opts.sample_rate)
    if opts.ntsc then
      table.insert(args, "--ntsc")
    end
  else
    table.insert(args, "--tempo")
    table.insert(args, tostring(opts.tempo))
    table.insert(args, "--ticks")
    table.insert(args, tostring(opts.ticks))
  end

  local command = table.concat(args, " ")
  local exit_ok = execute_succeeded(os.execute(command))

  local log_text = read_file(log_path) or ""
  os.remove(log_path)

  local had_error = log_text:find("%[ERROR%]") ~= nil
  -- The CLI logs a final "wrote N bytes..." confirmation on success (in addition to stdout) --
  -- an extra, independent success signal on top of os.execute's return value, since that value's
  -- meaning isn't fully certain across every Lua build Renoise might embed.
  local wrote_confirmation = log_text:find("%[INFO%] wrote %d+ bytes") ~= nil

  return (exit_ok or wrote_confirmation) and not had_error, output_path, log_text
end

local function show_export_dialog()
  local song = renoise.song()

  if song.file_name == "" then
    renoise.app():show_error(
      "Please save your song first -- Renoise2Mod converts the song file on disk, " ..
      "so it needs to exist before it can be exported."
    )
    return
  end

  local xrns_path = song.file_name
  local vb = renoise.ViewBuilder()
  local DIALOG_MARGIN = renoise.ViewBuilder.DEFAULT_DIALOG_MARGIN
  local CONTROL_SPACING = renoise.ViewBuilder.DEFAULT_CONTROL_SPACING

  local chosen_output_path = default_output_path(xrns_path, "xm")

  local ptmode_items = { "Hardware", "Software", "None" }
  local ptmode_values = { "hardware", "software", "none" }
  local volscal_items_mod = { "Sample", "None" }
  local volscal_values_mod = { "sample", "none" }
  local volscal_items_xm = { "Sample", "None", "Column" }
  local volscal_values_xm = { "sample", "none", "column" }

  local function update_visibility()
    local is_mod = vb.views.format_switch.value == 1
    vb.views.mod_options.visible = is_mod
    vb.views.xm_options.visible = not is_mod

    local volscal_items = is_mod and volscal_items_mod or volscal_items_xm
    vb.views.volscal_popup.items = volscal_items
    if vb.views.volscal_popup.value > #volscal_items then
      vb.views.volscal_popup.value = 1
    end
  end

  local content = vb:column {
    margin = DIALOG_MARGIN,
    spacing = CONTROL_SPACING,
    views = {
      vb:text { text = ("Song: %s"):format(xrns_path:match("[^/\\]+$") or xrns_path) },

      vb:row {
        spacing = CONTROL_SPACING,
        views = {
          vb:text { text = "Format:", width = 80 },
          vb:switch {
            id = "format_switch",
            items = { "MOD", "XM" },
            value = 2,
            width = 150,
            notifier = update_visibility,
          },
        },
      },

      vb:row {
        spacing = CONTROL_SPACING,
        views = {
          vb:text { text = "Volume Scaling:", width = 80 },
          vb:popup {
            id = "volscal_popup",
            items = volscal_items_xm,
            value = 1,
            width = 150,
          },
        },
      },

      vb:column {
        id = "xm_options",
        spacing = CONTROL_SPACING,
        views = {
          vb:row {
            spacing = CONTROL_SPACING,
            views = {
              vb:text { text = "Tempo:", width = 80 },
              vb:valuebox {
                id = "tempo_box",
                min = 0,
                max = 255,
                value = song.transport.bpm,
                width = 150,
                tooltip = "0 = use the song's own tempo",
              },
            },
          },
          vb:row {
            spacing = CONTROL_SPACING,
            views = {
              vb:text { text = "Ticks/Row:", width = 80 },
              vb:valuebox {
                id = "ticks_box",
                min = 0,
                max = 31,
                value = song.transport.tpl,
                width = 150,
                tooltip = "0 = use the song's own value",
              },
            },
          },
        },
      },

      vb:column {
        id = "mod_options",
        visible = false,
        spacing = CONTROL_SPACING,
        views = {
          vb:row {
            spacing = CONTROL_SPACING,
            views = {
              vb:text { text = "PT Compat:", width = 80 },
              vb:popup {
                id = "ptmode_popup",
                items = ptmode_items,
                value = 1,
                width = 150,
              },
            },
          },
          vb:row {
            spacing = CONTROL_SPACING,
            views = {
              vb:text { text = "NTSC:", width = 80 },
              vb:checkbox { id = "ntsc_checkbox", value = false },
            },
          },
          vb:row {
            spacing = CONTROL_SPACING,
            views = {
              vb:text { text = "Portamento:", width = 80 },
              vb:valuebox { id = "portresh_box", min = 0, max = 4, value = 2, width = 150 },
            },
          },
          vb:row {
            spacing = CONTROL_SPACING,
            views = {
              vb:text { text = "Sample Rate:", width = 80 },
              vb:textfield {
                id = "sample_rate_field",
                text = "low",
                width = 150,
                tooltip = "low, high, maximum, original, a note like C-2, or a Hz value",
              },
            },
          },
        },
      },
    },
  }

  local buttons = { "Export", "Cancel" }
  local pressed = renoise.app():show_custom_prompt("Renoise2Mod", content, buttons)

  if pressed ~= "Export" then
    return
  end

  local is_mod = vb.views.format_switch.value == 1
  local format = is_mod and "mod" or "xm"

  local volscal_items = is_mod and volscal_items_mod or volscal_items_xm
  local volscal_values = is_mod and volscal_values_mod or volscal_values_xm
  local volscal = volscal_values[vb.views.volscal_popup.value] or volscal_values[1]

  chosen_output_path = default_output_path(xrns_path, format)

  local opts = {
    format = format,
    volscal = volscal,
    ptmode = ptmode_values[vb.views.ptmode_popup.value] or "hardware",
    ntsc = vb.views.ntsc_checkbox.value,
    portresh = vb.views.portresh_box.value,
    sample_rate = vb.views.sample_rate_field.text,
    tempo = vb.views.tempo_box.value,
    ticks = vb.views.ticks_box.value,
  }

  renoise.app():show_status("Renoise2Mod: converting...")

  local success, output_path, log_text = run_export(xrns_path, chosen_output_path, opts)

  if success then
    renoise.app():show_status(("Renoise2Mod: wrote %s"):format(output_path))
  else
    local details = log_text ~= "" and log_text or "The bundled renoise2mod binary did not produce output. Check that it exists and is executable at:\n" .. binary_path()
    renoise.app():show_error("Renoise2Mod export failed:\n\n" .. details)
  end
end

renoise.tool():add_menu_entry {
  name = "Main Menu:File:Export to MOD/XM...",
  invoke = show_export_dialog,
}
