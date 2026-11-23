package basalt.oracle;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.mojang.blaze3d.vertex.PoseStack;
import net.minecraft.client.Minecraft;
import net.minecraft.client.model.EntityModel;
import net.minecraft.client.model.geom.ModelPart;
import net.minecraft.client.renderer.entity.EntityRenderer;
import net.minecraft.client.renderer.entity.LivingEntityRenderer;
import net.minecraft.client.renderer.entity.state.EntityRenderState;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.LivingEntity;
import org.joml.Matrix4f;

import java.io.BufferedWriter;
import java.io.IOException;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.Map;

/**
 * Test-only animation recorders (control ops "bones" and "fp", SCHEMA.md). Everything here calls
 * public client API found with `javap -p` (renderer lookup, createRenderState, Model.setupAnim,
 * ModelPart fields) plus reflection on field/method names (ModelPart.children, the renderer's
 * protected setupRotations/scale, ClientAvatarState/ItemInHandRenderer private fields). No Mojang
 * method bodies were read; the recorders only observe the client's own outputs.
 */
public final class Anim {
    private Anim() {}

    // ------------------------------------------------------------------ bones: per-tick model part poses

    private static BufferedWriter bones;
    private static String bonesType;   // entity type id ("minecraft:zombie"), "self", or null with bonesId
    private static int bonesId = -1;
    private static float[] partials = { 1.0f };

    static void bonesOp(Minecraft mc, JsonObject req, JsonObject o) throws IOException {
        if (bones != null) { bones.close(); bones = null; }
        if (req.has("file")) {
            bones = Files.newBufferedWriter(Paths.get(req.get("file").getAsString()), StandardCharsets.UTF_8);
            bonesType = req.has("type") ? req.get("type").getAsString() : null;
            if (bonesType != null && !bonesType.contains(":") && !bonesType.equals("self")) bonesType = "minecraft:" + bonesType;
            bonesId = req.has("entity") ? req.get("entity").getAsInt() : -1;
            if (req.has("partials")) {
                var a = req.getAsJsonArray("partials");
                partials = new float[a.size()];
                for (int i = 0; i < a.size(); i++) partials[i] = a.get(i).getAsFloat();
            } else partials = new float[] { 1.0f };
        }
    }

    private static Entity target(Minecraft mc) {
        if (mc.level == null || mc.player == null) return null;
        if ("self".equals(bonesType)) return mc.player;
        Entity best = null; double bd = Double.MAX_VALUE;
        for (Entity e : mc.level.entitiesForRendering()) {
            if (e == mc.player) continue;
            if (bonesId >= 0 ? e.getId() != bonesId : !BuiltInRegistries.ENTITY_TYPE.getKey(e.getType()).toString().equals(bonesType)) continue;
            double d = e.distanceToSqr(mc.player);
            if (d < bd) { bd = d; best = e; }
        }
        return best;
    }

    /** Called at the start of every client tick (after Control's queue ran). */
    static void onTick(Minecraft mc) {
        if (bones == null) return;
        try {
            Entity e = target(mc);
            JsonObject line = new JsonObject();
            line.addProperty("tick", Control.tick);
            if (e == null) { line.addProperty("missing", true); bones.write(line + "\n"); return; }
            line.addProperty("id", e.getId());
            line.addProperty("type", BuiltInRegistries.ENTITY_TYPE.getKey(e.getType()).toString());
            JsonArray pos = new JsonArray(); pos.add(e.getX()); pos.add(e.getY()); pos.add(e.getZ()); line.add("pos", pos);
            if (e instanceof LivingEntity le) {
                JsonObject raw = new JsonObject();
                raw.addProperty("hurtTime", le.hurtTime);
                raw.addProperty("deathTime", le.deathTime);
                raw.addProperty("swingTime", le.swingTime);
                raw.addProperty("attackAnim", le.attackAnim);
                raw.addProperty("yBodyRot", le.yBodyRot);
                raw.addProperty("yHeadRot", le.yHeadRot);
                raw.addProperty("yRot", le.getYRot());
                raw.addProperty("xRot", le.getXRot());
                raw.addProperty("walkSpeed", le.walkAnimation.speed());
                raw.addProperty("walkPos", le.walkAnimation.position());
                line.add("raw", raw);
            }
            JsonArray frames = new JsonArray();
            for (float pt : partials) frames.add(frame(mc, e, pt));
            line.add("frames", frames);
            bones.write(line + "\n");
            bones.flush();
        } catch (Throwable t) { t.printStackTrace(); }
    }

    @SuppressWarnings({ "unchecked", "rawtypes" })
    private static JsonObject frame(Minecraft mc, Entity e, float pt) throws Exception {
        JsonObject f = new JsonObject();
        f.addProperty("pt", pt);
        EntityRenderer r = mc.getEntityRenderDispatcher().getRenderer(e);
        EntityRenderState st = r.createRenderState(e, pt);
        f.add("state", fields(st));
        if (r instanceof LivingEntityRenderer<?, ?, ?> lr) {
            EntityModel m = lr.getModel();
            m.setupAnim(st);
            f.add("parts", parts(m.root()));
            // the renderer's own body transform (body yaw, death tilt, scale), from a fresh stack
            PoseStack ps = new PoseStack();
            float bodyRot = st instanceof net.minecraft.client.renderer.entity.state.LivingEntityRenderState ls ? ls.bodyRot : 0f;
            float scale = st instanceof net.minecraft.client.renderer.entity.state.LivingEntityRenderState ls2 ? ls2.scale : 1f;
            Method sr = find(lr.getClass(), "setupRotations", 4);
            sr.setAccessible(true);
            sr.invoke(lr, st, ps, bodyRot, scale);
            f.add("rot_matrix", mat(ps.last().pose()));
            Method sc = find(lr.getClass(), "scale", 2);
            sc.setAccessible(true);
            sc.invoke(lr, st, ps);
            f.add("body_matrix", mat(ps.last().pose()));
            f.addProperty("overlay", LivingEntityRenderer.getOverlayCoords((net.minecraft.client.renderer.entity.state.LivingEntityRenderState) st, 0f));
        }
        return f;
    }

    private static Method find(Class<?> c, String name, int n) throws NoSuchMethodException {
        for (Class<?> k = c; k != null; k = k.getSuperclass())
            for (Method m : k.getDeclaredMethods())
                if (m.getName().equals(name) && m.getParameterCount() == n && !m.isBridge()) return m;
        for (Class<?> k = c; k != null; k = k.getSuperclass())
            for (Method m : k.getDeclaredMethods())
                if (m.getName().equals(name) && m.getParameterCount() == n) return m;
        throw new NoSuchMethodException(name);
    }

    static JsonArray mat(Matrix4f m) {
        JsonArray a = new JsonArray();
        float[] v = new float[16];
        m.get(v); // column-major
        for (float x : v) a.add(x);
        return a;
    }

    private static Field CHILDREN;

    @SuppressWarnings("unchecked")
    private static JsonObject parts(ModelPart root) throws Exception {
        if (CHILDREN == null) { CHILDREN = ModelPart.class.getDeclaredField("children"); CHILDREN.setAccessible(true); }
        JsonObject out = new JsonObject();
        walk(root, "", out);
        return out;
    }

    @SuppressWarnings("unchecked")
    private static void walk(ModelPart p, String path, JsonObject out) throws Exception {
        for (Map.Entry<String, ModelPart> c : ((Map<String, ModelPart>) CHILDREN.get(p)).entrySet()) {
            ModelPart q = c.getValue();
            String name = path.isEmpty() ? c.getKey() : path + "/" + c.getKey();
            JsonArray a = new JsonArray();
            for (float v : new float[] { q.x, q.y, q.z, q.xRot, q.yRot, q.zRot, q.xScale, q.yScale, q.zScale }) a.add(v);
            a.add(q.visible && !q.skipDraw);
            out.add(name, a);
            walk(q, name, out);
        }
    }

    /** Public primitive/enum fields of a render state (whole class hierarchy). */
    private static JsonObject fields(Object st) {
        JsonObject o = new JsonObject();
        for (Class<?> k = st.getClass(); k != null && k != Object.class; k = k.getSuperclass()) {
            for (Field fl : k.getDeclaredFields()) {
                if (Modifier.isStatic(fl.getModifiers()) || !Modifier.isPublic(fl.getModifiers())) continue;
                try {
                    Class<?> t = fl.getType();
                    Object v = fl.get(st);
                    if (t == float.class || t == double.class || t == int.class) o.addProperty(fl.getName(), (Number) v);
                    else if (t == boolean.class) o.addProperty(fl.getName(), (Boolean) v);
                    else if (v instanceof Enum<?> en) o.addProperty(fl.getName(), en.name());
                } catch (Throwable ignored) {}
            }
        }
        return o;
    }

    // ------------------------------------------------------------------ fp: per-frame first-person transforms

    private static BufferedWriter fp;
    private static long fpFrame;

    static void fpOp(JsonObject req, JsonObject o) throws IOException {
        if (fp != null) { fp.close(); fp = null; }
        if (req.has("file")) fp = Files.newBufferedWriter(Paths.get(req.get("file").getAsString()), StandardCharsets.UTF_8);
    }

    private static Field f(Class<?> c, String n) throws NoSuchFieldException { Field x = c.getDeclaredField(n); x.setAccessible(true); return x; }

    /** One record: kind = "bob" (GameRenderer.bobView done), "item" (ItemInHandRenderer.renderItem, first person), "arm" (AvatarRenderer.renderRightHand). */
    public static void fpRecord(String kind, PoseStack ps, String extra) {
        if (fp == null) return;
        try {
            Minecraft mc = Minecraft.getInstance();
            if (mc.player == null) return;
            float pt = mc.getDeltaTracker().getGameTimeDeltaPartialTick(true);
            JsonObject l = new JsonObject();
            l.addProperty("tick", Control.tick);
            l.addProperty("frame", fpFrame);
            l.addProperty("pt", pt);
            l.addProperty("kind", kind);
            if (extra != null) l.addProperty("ctx", extra);
            l.add("m", mat(ps.last().pose()));
            var p = mc.player;
            l.addProperty("attack", p.getAttackAnim(pt));
            l.addProperty("attackAnim", p.attackAnim);
            l.addProperty("swingTime", p.swingTime);
            var av = p.avatarState();
            l.addProperty("walkDist", av.getInterpolatedWalkDistance(pt));
            l.addProperty("walkDistB", av.getBackwardsInterpolatedWalkDistance(pt));
            l.addProperty("bob", av.getInterpolatedBob(pt));
            var ih = mc.gameRenderer.itemInHandRenderer;
            l.addProperty("mainH", f(ih.getClass(), "mainHandHeight").getFloat(ih));
            l.addProperty("mainHO", f(ih.getClass(), "oMainHandHeight").getFloat(ih));
            l.addProperty("xRot", p.getXRot());
            l.addProperty("yRot", p.getYRot());
            l.addProperty("xBob", p.xBob); l.addProperty("yBob", p.yBob); l.addProperty("xBobO", p.xBobO); l.addProperty("yBobO", p.yBobO);
            fp.write(l + "\n");
        } catch (Throwable t) { t.printStackTrace(); }
    }

    /** A model part pose drawn in first person: x y z (px) and rotations (rad). */
    public static void fpPart(String kind, float[] pose) {
        if (fp == null) return;
        try {
            JsonObject l = new JsonObject();
            l.addProperty("tick", Control.tick);
            l.addProperty("frame", fpFrame);
            l.addProperty("kind", kind);
            JsonArray a = new JsonArray();
            for (float v : pose) a.add(v);
            l.add("part", a);
            fp.write(l + "\n");
        } catch (Throwable t) { t.printStackTrace(); }
    }

    // ------------------------------------------------------------------ frames: rendered frames as PNG

    private static java.nio.file.Path framesDir;
    private static double framesEvery;
    private static long framesLast, framesN;
    private static final java.util.concurrent.ExecutorService WRITER = java.util.concurrent.Executors.newSingleThreadExecutor(r -> { Thread t = new Thread(r, "basalt-oracle-frames"); t.setDaemon(true); return t; });

    /** {"op":"frames","dir":D,"fps":20} starts, {"op":"frames"} stops. */
    static void framesOp(JsonObject req, JsonObject o) throws IOException {
        framesDir = null;
        if (req.has("dir")) {
            framesDir = Paths.get(req.get("dir").getAsString());
            Files.createDirectories(framesDir);
            framesEvery = 1e9 / (req.has("fps") ? req.get("fps").getAsDouble() : 20.0);
            framesLast = 0; framesN = 0;
        }
    }

    private static void grabFrame() {
        if (framesDir == null) return;
        long now = System.nanoTime();
        if (now - framesLast < framesEvery) return;
        framesLast = now;
        java.nio.file.Path f = framesDir.resolve(String.format("f%05d_t%05d.png", framesN++, Control.tick));
        net.minecraft.client.Screenshot.takeScreenshot(Minecraft.getInstance().gameRenderer.mainRenderTarget(), img -> WRITER.submit(() -> {
            try { img.writeToFile(f); } catch (IOException e) { e.printStackTrace(); } finally { img.close(); }
        }));
    }

    public static void frameDone() {
        grabFrame(); if (fp != null) { fpFrame++; try { fp.flush(); } catch (IOException ignored) {} } }
}
