package basalt.oracle;

import com.google.gson.JsonObject;
import com.google.gson.stream.JsonWriter;

/** One oracle subsystem. A handler gets the parsed request and streams its response body
 *  (the fields after "id"/"kind") into an already-open JSON object. To add a subsystem:
 *  implement this, register it in OracleMod.HANDLERS, document it in SCHEMA.md. */
public interface Handler {
    void handle(JsonObject request, JsonWriter out) throws Exception;
}
