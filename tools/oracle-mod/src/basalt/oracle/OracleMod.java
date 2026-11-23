package basalt.oracle;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.google.gson.stream.JsonWriter;
import net.fabricmc.api.ClientModInitializer;
import net.minecraft.client.Minecraft;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;

import java.io.BufferedWriter;
import java.io.PrintWriter;
import java.io.StringWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.util.*;

/**
 * Test-only oracle. Watches DIR/in/*.json (DIR = -Dbasalt.oracle.dir), dispatches each request
 * by its "kind" to a Handler and writes DIR/out/<id>.json (atomically, via a .tmp rename).
 * Writes DIR/out/_ready.json once the client's resources are loaded.
 * Everything here only CALLS the client's public API; nothing is copied from it.
 */
public final class OracleMod implements ClientModInitializer {
    static final Map<String, Handler> HANDLERS = new LinkedHashMap<>();
    static {
        HANDLERS.put("ping", (req, out) -> out.name("pong").value(true));
        HANDLERS.put("mesh", new MeshHandler());
    }

    @Override public void onInitializeClient() {
        Path dir = Paths.get(System.getProperty("basalt.oracle.dir", "oracle"));
        Thread t = new Thread(() -> run(dir), "basalt-oracle");
        t.setDaemon(true);
        t.start();
    }

    static boolean ready() {
        try {
            Minecraft mc = Minecraft.getInstance();
            if (mc == null || mc.getModelManager() == null) return false;
            var set = mc.getModelManager().getBlockStateModelSet();
            if (set == null) return false;
            var stone = BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:stone")).defaultBlockState();
            return set.get(stone) != set.missingModel();
        } catch (Throwable e) { return false; }
    }

    static void run(Path dir) {
        Path in = dir.resolve("in"), out = dir.resolve("out"), done = in.resolve("done");
        try {
            Files.createDirectories(in); Files.createDirectories(out); Files.createDirectories(done);
            while (!ready()) Thread.sleep(500);
            Thread.sleep(2000); // let the first reload settle
            Files.writeString(out.resolve("_ready.json"), "{\"ready\":true,\"handlers\":" + new com.google.gson.Gson().toJson(HANDLERS.keySet()) + "}");
            System.out.println("[basalt-oracle] ready, watching " + in.toAbsolutePath());
            while (true) {
                List<Path> reqs = new ArrayList<>();
                try (DirectoryStream<Path> ds = Files.newDirectoryStream(in, "*.json")) { ds.forEach(reqs::add); }
                Collections.sort(reqs);
                for (Path p : reqs) process(p, out, done);
                if (reqs.isEmpty()) Thread.sleep(200);
            }
        } catch (Throwable e) {
            e.printStackTrace();
        }
    }

    static void process(Path p, Path outDir, Path done) throws Exception {
        String text = Files.readString(p, StandardCharsets.UTF_8);
        JsonObject req;
        try { req = JsonParser.parseString(text).getAsJsonObject(); }
        catch (Exception e) { Thread.sleep(100); return; } // probably still being written; retry
        String id = req.has("id") ? req.get("id").getAsString() : p.getFileName().toString().replace(".json", "");
        String kind = req.has("kind") ? req.get("kind").getAsString() : "";
        Path tmp = outDir.resolve(id + ".json.tmp");
        long t0 = System.nanoTime();
        try (BufferedWriter bw = Files.newBufferedWriter(tmp, StandardCharsets.UTF_8); JsonWriter w = new JsonWriter(bw)) {
            w.beginObject();
            w.name("id").value(id);
            w.name("kind").value(kind);
            w.name("oracle").value("vanilla-26.2");
            Handler h = HANDLERS.get(kind);
            if (h == null) w.name("error").value("unknown kind '" + kind + "'; known: " + HANDLERS.keySet());
            else {
                try { h.handle(req, w); }
                catch (Throwable e) { StringWriter sw = new StringWriter(); e.printStackTrace(new PrintWriter(sw)); w.name("error").value(sw.toString()); }
            }
            w.name("millis").value((System.nanoTime() - t0) / 1_000_000);
            w.endObject();
        }
        Files.move(tmp, outDir.resolve(id + ".json"), StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
        Files.move(p, done.resolve(p.getFileName()), StandardCopyOption.REPLACE_EXISTING);
        System.out.println("[basalt-oracle] " + kind + " " + id + " in " + (System.nanoTime() - t0) / 1_000_000 + " ms");
    }
}
