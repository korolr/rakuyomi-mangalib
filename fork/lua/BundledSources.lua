-- rakuyomi-mangalib: installs the sources shipped with the plugin (the MangaLib
-- source with token support) into the server's `sources` folder, so they are
-- available right after installing the plugin, without a source list.
local lfs = require("libs/libkoreader-lfs")
local logger = require("logger")
local Paths = require("Paths")

local BundledSources = {}

local function readFile(path)
  local f = io.open(path, "rb")
  if not f then return nil end
  local data = f:read("*a")
  f:close()
  return data
end

local function ensureDirectory(path)
  if lfs.attributes(path, "mode") ~= "directory" then
    lfs.mkdir(path)
  end
end

--- Copies every `bundled-sources/*.aix` file into `<home>/sources/` when it is
--- missing there or differs from the bundled one.
function BundledSources.install()
  local bundled_dir = Paths.getPluginDirectory() .. "/bundled-sources"
  if lfs.attributes(bundled_dir, "mode") ~= "directory" then
    return
  end

  local home = Paths.getHomeDirectory()
  ensureDirectory(home)
  local sources_dir = home .. "/sources"
  ensureDirectory(sources_dir)

  for name in lfs.dir(bundled_dir) do
    if name:match("%.aix$") then
      local bundled = readFile(bundled_dir .. "/" .. name)
      local target_path = sources_dir .. "/" .. name
      if bundled and readFile(target_path) ~= bundled then
        local tmp_path = target_path .. ".tmp"
        local f = io.open(tmp_path, "wb")
        if f then
          f:write(bundled)
          f:close()
          os.rename(tmp_path, target_path)
          logger.info("rakuyomi-mangalib: installed bundled source", name)
        else
          logger.warn("rakuyomi-mangalib: could not install bundled source", name)
        end
      end
    end
  end
end

return BundledSources
