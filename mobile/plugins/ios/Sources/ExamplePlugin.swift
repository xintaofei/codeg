import SwiftRs
import Security
import Tauri
import UIKit
import WebKit

class SecretKeyArgs: Decodable {
  let key: String
}

class SecretValueArgs: Decodable {
  let key: String
  let value: String
}

class SecureVaultPlugin: Plugin {
  private let service = "cn.crain.codeg.mobile.secure-vault"

  private func baseQuery(key: String) -> [String: Any] {
    return [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: key,
    ]
  }

  @objc public func storeSecret(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(SecretValueArgs.self)
    guard let data = args.value.data(using: .utf8) else {
      invoke.reject("Secret is not valid UTF-8")
      return
    }
    var query = baseQuery(key: args.key)
    SecItemDelete(query as CFDictionary)
    query[kSecValueData as String] = data
    query[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    let status = SecItemAdd(query as CFDictionary, nil)
    if status == errSecSuccess {
      invoke.resolve()
    } else {
      invoke.reject("Keychain write failed (\(status))")
    }
  }

  @objc public func loadSecret(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(SecretKeyArgs.self)
    var query = baseQuery(key: args.key)
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    var result: AnyObject?
    let status = SecItemCopyMatching(query as CFDictionary, &result)
    if status == errSecItemNotFound {
      invoke.resolve(["value": NSNull()])
      return
    }
    guard status == errSecSuccess,
          let data = result as? Data,
          let value = String(data: data, encoding: .utf8) else {
      invoke.reject("Keychain read failed (\(status))")
      return
    }
    invoke.resolve(["value": value])
  }

  @objc public func deleteSecret(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(SecretKeyArgs.self)
    let status = SecItemDelete(baseQuery(key: args.key) as CFDictionary)
    if status == errSecSuccess || status == errSecItemNotFound {
      invoke.resolve()
    } else {
      invoke.reject("Keychain delete failed (\(status))")
    }
  }
}

@_cdecl("init_plugin_secure_vault")
func initPlugin() -> Plugin {
  return SecureVaultPlugin()
}
