package basalt.oracle.mixin;

import basalt.oracle.PacketLog;
import com.google.gson.JsonObject;
import net.minecraft.client.particle.Particle;
import net.minecraft.client.particle.ParticleEngine;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

import net.minecraft.client.particle.SingleQuadParticle;
import net.minecraft.client.renderer.texture.TextureAtlasSprite;
import java.lang.reflect.Field;
import java.util.IdentityHashMap;
import java.util.Map;
import java.util.Queue;
import java.util.Set;

/**
 * Test-only: logs the particles the client spawns locally (class, position, velocity, lifetime) as
 * {"tick","fx":"particle",...} lines in the packet log, so the effects the client makes by itself
 * (splashes, bubbles) can be compared with basalt-client's.
 */
@Mixin(ParticleEngine.class)
public abstract class FxParticleMixin {
    private static Field f(String n) {
        try { Field x = Particle.class.getDeclaredField(n); x.setAccessible(true); return x; } catch (Throwable t) { return null; }
    }
    private static final Field X = f("x"), Y = f("y"), Z = f("z"), XD = f("xd"), YD = f("yd"), ZD = f("zd"), LIFE = f("lifetime"), GRAV = f("gravity"), FRIC = f("friction");

    private static Field sf(String n) {
        try { Field x = SingleQuadParticle.class.getDeclaredField(n); x.setAccessible(true); return x; } catch (Throwable t) { return null; }
    }
    private static final Field QSIZE = sf("quadSize"), SPRITE = sf("sprite"), ALPHA = sf("alpha");
    private static final Set<String> AMBIENT = Set.of("SuspendedParticle", "DripHangParticle", "FallAndLandParticle", "SuspendedTownParticle");
    private static final Map<Object, Integer> IDS = new IdentityHashMap<>();
    private static int nextId = 1;

    private static String sprite(Particle p) {
        try { if (SPRITE != null && p instanceof SingleQuadParticle) { Object s = SPRITE.get(p); if (s instanceof TextureAtlasSprite t) return t.contents().name().toString(); } } catch (Throwable t) {}
        return "";
    }

    /** Every tick: positions of the live non-ambient particles (id = creation order), for motion fits. */
    @Inject(method = "tick", at = @At("TAIL"))
    private void basalt$tick(CallbackInfo ci) {
        try {
            Field pf = ParticleEngine.class.getDeclaredField("particles"); pf.setAccessible(true);
            Map<?, ?> groups = (Map<?, ?>) pf.get(this);
            Field qf = Class.forName("net.minecraft.client.particle.ParticleGroup").getDeclaredField("particles"); qf.setAccessible(true);
            for (Object g : groups.values()) {
                for (Object o : (Queue<?>) qf.get(g)) {
                    Particle p = (Particle) o;
                    String n = p.getClass().getSimpleName();
                    if (AMBIENT.contains(n)) continue;
                    Integer id = IDS.get(p);
                    if (id == null) { id = nextId++; IDS.put(p, id); }
                    JsonObject j = new JsonObject();
                    j.addProperty("fx", "ptick"); j.addProperty("name", n); j.addProperty("id", id);
                    j.addProperty("x", d(X, p)); j.addProperty("y", d(Y, p)); j.addProperty("z", d(Z, p));
                    j.addProperty("vx", d(XD, p)); j.addProperty("vy", d(YD, p)); j.addProperty("vz", d(ZD, p));
                    j.addProperty("size", QSIZE == null ? 0 : fl(QSIZE, p));
                    j.addProperty("sprite", sprite(p));
                    PacketLog.fx(j);
                }
            }
            if (IDS.size() > 5000) IDS.clear();
        } catch (Throwable t) {}
    }

    private static double d(Field f, Object o) { try { return f == null ? 0 : f.getDouble(o); } catch (Throwable t) { return 0; } }
    private static float fl(Field f, Object o) { try { return f == null ? 0 : f.getFloat(o); } catch (Throwable t) { return 0; } }
    private static int in(Field f, Object o) { try { return f == null ? 0 : f.getInt(o); } catch (Throwable t) { return 0; } }

    @Inject(method = "add", at = @At("HEAD"))
    private void basalt$add(Particle p, CallbackInfo ci) {
        JsonObject o = new JsonObject();
        o.addProperty("fx", "particle");
        o.addProperty("name", p.getClass().getSimpleName());
        o.addProperty("x", d(X, p)); o.addProperty("y", d(Y, p)); o.addProperty("z", d(Z, p));
        o.addProperty("vx", d(XD, p)); o.addProperty("vy", d(YD, p)); o.addProperty("vz", d(ZD, p));
        o.addProperty("life", in(LIFE, p));
        o.addProperty("size", QSIZE == null ? 0 : fl(QSIZE, p));
        o.addProperty("sprite", sprite(p));
        if (!AMBIENT.contains(p.getClass().getSimpleName())) { Integer id = IDS.get(p); if (id == null) { id = nextId++; IDS.put(p, id); } o.addProperty("id", id); }
        o.addProperty("gravity", fl(GRAV, p));
        o.addProperty("friction", fl(FRIC, p));
        PacketLog.fx(o);
    }
}
